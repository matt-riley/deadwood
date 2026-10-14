package main

import (
	"context"
	"testing"
)

func TestOpaqueInterfaceAndFunctionResultEscapes(t *testing.T) {
	root := fixture(t, map[string]string{"go.mod": "module example.test/probe\ngo 1.26\n", "main.go": `package main
import "fmt"
type S struct{}
func (S) String() string {return format()}
func format() string {return "value"}
func main(){var value any=S{};fmt.Println(value);fmt.Println(func() any{return S{}})}
`})
	check(t, runGraph(t, root, false), map[string]bool{"S.String": true, "format": true})
}
func TestCgoPublicAPIKeepsOrdinaryGoHelpers(t *testing.T) {
	root := fixture(t, map[string]string{"go.mod": "module example.test/probe\ngo 1.26\n", "lib.go": "package probe\nfunc helper(){}\n",
		"cgo.go": `package probe
/* int value() {return 1;} */
import "C"
func Public() int {helper();return int(C.value())}
`})
	g := analyze(context.Background(), Request{Root: root, Dirs: []string{root}, Cgo: "1"})
	if !g.Complete {
		t.Fatalf("%+v", g.Diagnostics)
	}
	check(t, g, map[string]bool{"helper": true})
	req := Request{Root: root, Dirs: []string{root}, Cgo: "1", CheckExportFiles: map[string]bool{"cgo.go": true, "lib.go": true}}
	check(t, analyze(context.Background(), req), map[string]bool{"helper": false})
	req.Entries = []string{"cgo.go"}
	check(t, analyze(context.Background(), req), map[string]bool{"helper": true})
}
