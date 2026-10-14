package main

import (
	"context"
	"path/filepath"
	"testing"
)

func TestInstantiatedGenericInterfaceDispatch(t *testing.T) {
	root := fixture(t, map[string]string{"go.mod": "module example.test/probe\ngo 1.26\n", "main.go": `package main
type I interface {Get() int}
type Box[T any] struct{v T}
func (b Box[T]) Get() T {used();return b.v}
func used(){}
func call(i I){_=i.Get()}
func main(){call(Box[int]{})}
`})
	check(t, runGraph(t, root, false), map[string]bool{"Box.Get": true, "used": true})
}
func TestLocalReplaceClosureAndBlankImports(t *testing.T) {
	root := fixture(t, map[string]string{"go.mod": "module example.test/app\ngo 1.26\nrequire example.test/lib v0.0.0\nreplace example.test/lib => ./lib\n",
		"main.go":    "package main\nimport _ \"example.test/lib\"\nfunc main(){}\n",
		"lib/go.mod": "module example.test/lib\ngo 1.26\n",
		"lib/lib.go": "package lib\nfunc init(){used()}\nfunc used(){}\nfunc Dead(){}\n"})
	check(t, runGraph(t, root, false), map[string]bool{"used": true, "Dead": false})
}
func TestCanceledEmptyAndInvalidRequests(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	root := fixture(t, map[string]string{"go.mod": "module example.test/probe\ngo 1.26\n", "main.go": "package main\nfunc main(){}"})
	for _, req := range []Request{{}, {Root: root, Dirs: []string{root}, Target: "not-a-go-target"}} {
		g := analyze(context.Background(), req)
		if g.Complete || len(g.Diagnostics) == 0 {
			t.Fatalf("invalid request passed: %+v", g)
		}
	}
	g := analyze(ctx, Request{Root: root, Dirs: []string{root}})
	if g.Complete || len(g.Diagnostics) == 0 {
		t.Fatal("canceled compiler passed")
	}
	empty := fixture(t, map[string]string{"go.mod": "module example.test/empty\ngo 1.26\n"})
	g = analyze(context.Background(), Request{Root: empty, Dirs: []string{empty}})
	if g.Complete {
		t.Fatal("empty analysis passed")
	}
}
func TestEmbedAndCgoProfiles(t *testing.T) {
	root := fixture(t, map[string]string{"go.mod": "module example.test/probe\ngo 1.26\n", "main.go": `package main
import _ "embed"
//go:embed data.txt
var data string
func main(){_=data}
`, "data.txt": "asset", "cgo.go": `package main
/* int value() {return 1;} */
import "C"
func cgoValue() int {return int(C.value())}
`})
	req := Request{Root: root, Dirs: []string{root}, CheckExports: true, Cgo: "0"}
	g := analyze(context.Background(), req)
	if !g.Complete || g.Profile["cgo"] != "0" {
		t.Fatalf("%+v", g.Diagnostics)
	}
	seen := false
	for _, p := range g.Packages {
		for _, f := range p.Files {
			if f == "data.txt" {
				seen = true
			}
			if filepath.Base(f) == "cgo.go" {
				t.Fatal("cgo file loaded with CGO_ENABLED=0")
			}
		}
	}
	if !seen {
		t.Fatal("missing embedded asset")
	}
	req.Cgo = "1"
	g = analyze(context.Background(), req)
	if !g.Complete {
		t.Fatalf("cgo loading failed: %+v", g.Diagnostics)
	}
	if g.Profile["cgo"] != "1" {
		t.Fatal("wrong cgo profile")
	}
}
