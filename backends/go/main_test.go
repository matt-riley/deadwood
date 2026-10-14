package main

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func fixture(t *testing.T, files map[string]string) string {
	t.Helper()
	root := t.TempDir()
	for name, body := range files {
		path := filepath.Join(root, name)
		if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(body), 0600); err != nil {
			t.Fatal(err)
		}
	}
	return root
}
func runGraph(t *testing.T, root string, tests bool) Graph {
	t.Helper()
	g := analyze(context.Background(), Request{Root: root, Dirs: []string{root}, Tests: tests, CheckExports: true})
	if !g.Complete || len(g.Declarations) == 0 || len(g.Packages) == 0 || len(g.Roots) == 0 {
		t.Fatalf("empty/incomplete analysis: %+v", g.Diagnostics)
	}
	return g
}
func live(g Graph) map[string]bool {
	result := map[string]bool{}
	for _, r := range g.Roots {
		result[r.Symbol] = true
	}
	for changed := true; changed; {
		changed = false
		for _, e := range g.References {
			if result[e.From] && !result[e.To] {
				result[e.To] = true
				changed = true
			}
		}
	}
	return result
}
func check(t *testing.T, g Graph, want map[string]bool) {
	t.Helper()
	reachable := live(g)
	for name, expected := range want {
		count := 0
		for _, d := range g.Declarations {
			if d.Reportable && d.Name == name {
				count++
				if reachable[d.ID] != expected {
					t.Errorf("%s reachability = %v, want %v", name, reachable[d.ID], expected)
				}
			}
		}
		if count != 1 {
			t.Errorf("expected exactly 1 declaration for %s, got %d", name, count)
		}
	}
}
func TestCompilerIdentitiesCyclesShadowingAndSpans(t *testing.T) {
	root := fixture(t, map[string]string{"go.mod": "module example.test/probe\ngo 1.26\n", "main.go": `package main
type A struct{}
type B struct{}
func (A) ping(){ helper() }
func (B) ping(){ dead1() }
func helper(){}
func dead1(){ dead2() }
func dead2(){ dead1() }
func shadowed(){}
func main(){ shadowed:=func(){}; shadowed(); A{}.ping(); _=B{} }
`})
	g := runGraph(t, root, false)
	check(t, g, map[string]bool{"A.ping": true, "B.ping": false, "helper": true, "dead1": false, "dead2": false, "shadowed": false, "main": true})
	source, _ := os.ReadFile(filepath.Join(root, "main.go"))
	for _, d := range g.Declarations {
		if d.Reportable && d.Name == "B.ping" {
			if d.Span.File != "main.go" || !strings.Contains(string(source[d.Span.Start:d.Span.End]), "func (B) ping()") {
				t.Fatalf("wrong span: %+v", d.Span)
			}
		}
	}
}
func TestInitClosuresGenericsAndTypedInterfaceDispatch(t *testing.T) {
	root := fixture(t, map[string]string{"go.mod": "module example.test/probe\ngo 1.26\n", "main.go": `package main
type I interface { run() }
type A struct{}
type B struct{}
func (A) run(){ used() }
func (B) run(int){ dead() }
func used(){}
func dead(){}
func initialize() int { return 1 }
var unused = initialize()
func init(){ callback:=func(){used()};callback() }
func dispatch(i I){i.run()}
func identity[T any](v T) T {return v}
func main(){dispatch(A{}); _=identity(1)}
`})
	check(t, runGraph(t, root, false), map[string]bool{"A.run": true, "B.run": false, "used": true, "dead": false, "initialize": true, "unused": false, "identity": true})
}
func TestGeneratedTestHarnessAndProductionProfile(t *testing.T) {
	root := fixture(t, map[string]string{"go.mod": "module example.test/probe\ngo 1.26\n",
		"lib.go":      "package probe\nfunc used(){}\nfunc dead(){}\n",
		"lib_test.go": "package probe\nimport \"testing\"\nfunc TestWorks(t *testing.T){used()}\nfunc helper() {dead()}\n"})
	check(t, runGraph(t, root, true), map[string]bool{"used": true, "dead": false, "TestWorks": true, "helper": false})
	check(t, runGraph(t, root, false), map[string]bool{"used": false, "dead": false})
}
func TestProfilesWorkspacesAndCompilerFailures(t *testing.T) {
	root := fixture(t, map[string]string{"go.work": "go 1.26\nuse (\n ./app\n ./lib\n)\n",
		"app/go.mod":     "module example.test/app\ngo 1.26\nrequire example.test/lib v0.0.0\nreplace example.test/lib => ../lib\n",
		"app/main.go":    "package main\nimport \"example.test/lib\"\nfunc main(){lib.Used()}\n",
		"app/feature.go": "//go:build feature\n\npackage main\nfunc feature(){}\n",
		"lib/go.mod":     "module example.test/lib\ngo 1.26\n", "lib/lib.go": "package lib\nfunc Used(){}\nfunc Dead(){}\n"})
	req := Request{Root: root, Dirs: []string{filepath.Join(root, "app"), filepath.Join(root, "lib")}, CheckExports: true, Tags: []string{"feature"}, Target: "linux/amd64"}
	g := analyze(context.Background(), req)
	if !g.Complete || g.Profile["target"] != "linux/amd64" {
		t.Fatalf("%+v", g)
	}
	check(t, g, map[string]bool{"Used": true, "Dead": false, "feature": false})
	os.WriteFile(filepath.Join(root, "app/main.go"), []byte("package main\nfunc main(){doesNotExist()}"), 0600)
	bad := analyze(context.Background(), req)
	if bad.Complete || len(bad.Diagnostics) == 0 {
		t.Fatal("compiler error became clean")
	}
}
func TestPublicAPIBoundaryAndExternalMethodEscape(t *testing.T) {
	root := fixture(t, map[string]string{"go.mod": "module example.test/probe\ngo 1.26\n", "lib.go": "package probe\ntype Public struct{}\nfunc (Public) Method(){}\nfunc Exported(){}\nfunc dead(){}\n"})
	g := analyze(context.Background(), Request{Root: root, Dirs: []string{root}})
	check(t, g, map[string]bool{"Public": true, "Public.Method": true, "Exported": true, "dead": false})
	root = fixture(t, map[string]string{"go.mod": "module example.test/probe\ngo 1.26\n", "main.go": `package main
import "fmt"
type S struct{}
func (S) String() string {return format()}
func format() string {return "value"}
func main(){fmt.Println(S{})}
`})
	check(t, runGraph(t, root, false), map[string]bool{"S.String": true, "format": true})
}
