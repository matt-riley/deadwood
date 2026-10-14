// The helper is a compiler adapter, not a second user-facing CLI.
package main

import (
	"context"
	"encoding/json"
	"fmt"
	"go/ast"
	"go/token"
	"go/types"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"time"

	"golang.org/x/tools/go/packages"
)

type Span struct {
	File  string `json:"file"`
	Start int    `json:"start"`
	End   int    `json:"end"`
	Line  int    `json:"line"`
}
type Declaration struct {
	ID         string  `json:"id"`
	Name       string  `json:"name"`
	Kind       string  `json:"kind"`
	Span       Span    `json:"span"`
	Owner      *string `json:"owner"`
	Exported   bool    `json:"exported"`
	Reportable bool    `json:"reportable"`
}
type Reference struct {
	From string `json:"from"`
	To   string `json:"to"`
	Span Span   `json:"span"`
}
type Root struct {
	Symbol string `json:"symbol"`
	Reason string `json:"reason"`
}
type Package struct {
	ID      string   `json:"id"`
	Files   []string `json:"files"`
	Imports []string `json:"imports"`
}
type Diagnostic struct {
	Kind    string `json:"kind"`
	Message string `json:"message"`
}
type Graph struct {
	V            int               `json:"v"`
	Language     string            `json:"language"`
	Complete     bool              `json:"complete"`
	Profile      map[string]string `json:"profile"`
	Declarations []Declaration     `json:"declarations"`
	References   []Reference       `json:"references"`
	Roots        []Root            `json:"roots"`
	Packages     []Package         `json:"packages"`
	Diagnostics  []Diagnostic      `json:"diagnostics"`
}
type Request struct {
	Root             string          `json:"root"`
	Dirs             []string        `json:"dirs"`
	Tests            bool            `json:"tests"`
	Tags             []string        `json:"tags"`
	Target           string          `json:"target"`
	CheckExports     bool            `json:"check_exports"`
	Entries          []string        `json:"entries"`
	Timeout          uint64          `json:"timeout"`
	Cgo              string          `json:"cgo"`
	CheckExportFiles map[string]bool `json:"check_export_files"`
}
type unit struct {
	pkg    *packages.Package
	node   ast.Node
	owners []string
}
type builder struct {
	root  string
	fset  *token.FileSet
	graph Graph
	decls map[string]Declaration
	units []unit
	named []*types.Named
}

func main() {
	var req Request
	decoder := json.NewDecoder(io.LimitReader(os.Stdin, 1<<20))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&req); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	seconds := req.Timeout
	if seconds == 0 {
		seconds = 120
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Duration(seconds)*time.Second)
	defer cancel()
	if err := json.NewEncoder(os.Stdout).Encode(analyze(ctx, req)); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
func (b *builder) fail(message string) {
	b.graph.Complete = false
	b.graph.Diagnostics = append(b.graph.Diagnostics, Diagnostic{"semantic_error", message})
}
func (b *builder) path(path string) string {
	relative, err := filepath.Rel(b.root, path)
	if err == nil && relative != ".." && !strings.HasPrefix(relative, ".."+string(filepath.Separator)) {
		return filepath.ToSlash(relative)
	}
	return filepath.ToSlash(path)
}
func (b *builder) span(node ast.Node) Span {
	start := b.fset.PositionFor(node.Pos(), false)
	end := b.fset.PositionFor(node.End(), false)
	return Span{b.path(start.Filename), start.Offset, end.Offset, start.Line}
}
func origin(obj types.Object) types.Object {
	if fn, ok := obj.(*types.Func); ok {
		return fn.Origin()
	}
	return obj
}
func (b *builder) id(obj types.Object) string {
	if obj == nil || !obj.Pos().IsValid() {
		return ""
	}
	pos := b.fset.PositionFor(origin(obj).Pos(), false)
	return b.path(pos.Filename) + ":" + strconv.Itoa(pos.Offset)
}
func named(t types.Type) *types.Named {
	t = types.Unalias(t)
	if p, ok := t.(*types.Pointer); ok {
		t = types.Unalias(p.Elem())
	}
	n, _ := t.(*types.Named)
	return n
}
func (b *builder) add(obj types.Object, node ast.Node, reportable bool) string {
	id := b.id(obj)
	if id == "" {
		return ""
	}
	name := obj.Name()
	kind := "Value"
	var owner *string
	if _, ok := obj.(*types.TypeName); ok {
		kind = "Type"
		if n := named(obj.Type()); n != nil {
			b.named = append(b.named, n)
		}
	}
	if fn, ok := obj.(*types.Func); ok {
		if recv := fn.Type().(*types.Signature).Recv(); recv != nil {
			if n := named(recv.Type()); n != nil {
				name = n.Obj().Name() + "." + name
				v := b.id(n.Obj())
				owner = &v
			}
		}
	}
	span := b.span(node)
	// Generated compiler files participate in binding, but are not deletion candidates.
	reportable = reportable && !filepath.IsAbs(span.File)
	b.decls[id] = Declaration{id, name, kind, span, owner, obj.Exported(), reportable}
	return id
}
func (b *builder) edge(from, to string, span Span) {
	if from != "" && to != "" {
		b.graph.References = append(b.graph.References, Reference{from, to, span})
	}
}
func (b *builder) seed(id, reason string)  { b.graph.Roots = append(b.graph.Roots, Root{id, reason}) }
func packageID(p *packages.Package) string { return "package:" + p.PkgPath }
func (b *builder) collect(p *packages.Package) {
	pid := packageID(p)
	first := b.path(p.GoFiles[0])
	b.decls[pid] = Declaration{ID: pid, Name: p.PkgPath, Kind: "Value", Span: Span{first, 0, 0, 1}}
	for _, file := range p.Syntax {
		for _, decl := range file.Decls {
			switch d := decl.(type) {
			case *ast.FuncDecl:
				obj := p.TypesInfo.Defs[d.Name]
				if obj == nil {
					continue
				}
				id := b.add(obj, d, !strings.HasSuffix(p.PkgPath, ".test"))
				b.units = append(b.units, unit{p, d, []string{id}})
				if d.Recv == nil && d.Name.Name == "init" {
					b.edge(pid, id, b.span(d))
				}
				if d.Doc != nil {
					for _, comment := range d.Doc.List {
						if strings.HasPrefix(comment.Text, "//go:linkname ") || strings.HasPrefix(comment.Text, "//export ") {
							b.seed(id, "foreign linkage")
						}
					}
				}
			case *ast.GenDecl:
				var constants []string
				for _, spec := range d.Specs {
					switch s := spec.(type) {
					case *ast.TypeSpec:
						id := b.add(p.TypesInfo.Defs[s.Name], s, !strings.HasSuffix(p.PkgPath, ".test"))
						b.units = append(b.units, unit{p, s, []string{id}})
						ast.Inspect(s, func(n ast.Node) bool {
							if ident, ok := n.(*ast.Ident); ok {
								if obj, ok := p.TypesInfo.Defs[ident].(*types.Func); ok {
									mid := b.add(obj, ident, false)
									b.edge(id, mid, b.span(ident))
								}
							}
							return true
						})
					case *ast.ValueSpec:
						var owners []string
						for _, name := range s.Names {
							obj := p.TypesInfo.Defs[name]
							if obj == nil || name.Name == "_" {
								continue
							}
							owners = append(owners, b.add(obj, s, !strings.HasSuffix(p.PkgPath, ".test")))
						}
						if d.Tok == token.CONST {
							constants = append(constants, owners...)
						}
						if d.Tok == token.VAR && len(s.Values) > 0 {
							span := b.span(s)
							id := "initializer:" + span.File + ":" + strconv.Itoa(span.Start)
							b.decls[id] = Declaration{ID: id, Name: "initializer", Kind: "Value", Span: span}
							b.edge(pid, id, span)
							owners = append(owners, id)
						}
						b.units = append(b.units, unit{p, s, owners})
					}
				}
				// Const specs can inherit expressions/iota from previous specs.
				for i := 1; i < len(constants); i++ {
					span := b.decls[constants[i]].Span
					b.edge(constants[i], constants[i-1], span)
					b.edge(constants[i-1], constants[i], span)
				}
			}
		}
	}
}
func qualifier(p *types.Package) string { return p.Path() }
func matches(n types.Type, iface *types.Interface) bool {
	if types.Implements(n, iface) {
		return true
	}
	// Independently loaded workspace modules can carry distinct *types.Package identities.
	set := types.NewMethodSet(n)
	for i := 0; i < iface.NumMethods(); i++ {
		method := iface.Method(i)
		selection := set.Lookup(method.Pkg(), method.Name())
		if selection == nil || types.TypeString(selection.Obj().Type(), qualifier) != types.TypeString(method.Type(), qualifier) {
			return false
		}
	}
	return true
}
func (b *builder) dispatch(from string, selection *types.Selection, span Span) {
	iface, ok := selection.Recv().Underlying().(*types.Interface)
	if !ok {
		return
	}
	iface.Complete()
	for _, n := range b.named {
		for _, t := range []types.Type{n, types.NewPointer(n)} {
			if !matches(t, iface) {
				continue
			}
			method := types.NewMethodSet(t).Lookup(selection.Obj().Pkg(), selection.Obj().Name())
			if method != nil {
				b.edge(from, b.id(method.Obj()), span)
			}
		}
	}
}
func (b *builder) escape(from string, t types.Type, span Span, seen map[types.Type]bool) {
	if t == nil || seen[t] {
		return
	}
	seen[t] = true
	if n := named(t); n != nil {
		for _, typ := range []types.Type{n, types.NewPointer(n)} {
			methods := types.NewMethodSet(typ)
			for i := 0; i < methods.Len(); i++ {
				m := methods.At(i).Obj()
				if m.Exported() {
					b.edge(from, b.id(m), span)
				}
			}
		}
	}
	switch t := types.Unalias(t).Underlying().(type) {
	case *types.Pointer:
		b.escape(from, t.Elem(), span, seen)
	case *types.Slice:
		b.escape(from, t.Elem(), span, seen)
	case *types.Interface:
		// Without flow analysis an interface value may hold any compatible local type.
		for _, candidate := range b.named {
			if matches(candidate, t) {
				b.escape(from, candidate, span, seen)
			}
		}
	case *types.Signature:
		for i := 0; i < t.Results().Len(); i++ {
			b.escape(from, t.Results().At(i).Type(), span, seen)
		}
	case *types.Array:
		b.escape(from, t.Elem(), span, seen)
	case *types.Map:
		b.escape(from, t.Key(), span, seen)
		b.escape(from, t.Elem(), span, seen)
	case *types.Chan:
		b.escape(from, t.Elem(), span, seen)
	case *types.Struct:
		for i := 0; i < t.NumFields(); i++ {
			b.escape(from, t.Field(i).Type(), span, seen)
		}
	}
}
func callee(info *types.Info, expr ast.Expr) types.Object {
	switch e := expr.(type) {
	case *ast.Ident:
		return info.Uses[e]
	case *ast.SelectorExpr:
		return info.Uses[e.Sel]
	case *ast.IndexExpr:
		return callee(info, e.X)
	case *ast.IndexListExpr:
		return callee(info, e.X)
	case *ast.ParenExpr:
		return callee(info, e.X)
	}
	return nil
}
func (b *builder) references() {
	for _, u := range b.units {
		ast.Inspect(u.node, func(node ast.Node) bool {
			if node == nil {
				return true
			}
			span := b.span(node)
			for _, owner := range u.owners {
				switch n := node.(type) {
				case *ast.Ident:
					b.edge(owner, b.id(u.pkg.TypesInfo.Uses[n]), span)
				case *ast.SelectorExpr:
					if selection := u.pkg.TypesInfo.Selections[n]; selection != nil {
						b.dispatch(owner, selection, span)
					}
				case *ast.CallExpr:
					obj := callee(u.pkg.TypesInfo, n.Fun)
					// External callees may invoke callbacks/Stringer/reflective exported methods.
					if obj != nil && obj.Pkg() != nil && b.decls[b.id(obj)].ID == "" {
						for _, arg := range n.Args {
							b.escape(owner, u.pkg.TypesInfo.TypeOf(arg), span, map[types.Type]bool{})
						}
					}
				}
			}
			return true
		})
	}
	// References to stdlib/external declarations are outside the local cleanup graph.
	valid := b.graph.References[:0]
	for _, ref := range b.graph.References {
		if b.decls[ref.From].ID != "" && b.decls[ref.To].ID != "" {
			valid = append(valid, ref)
		}
	}
	b.graph.References = valid
}
func (b *builder) environment(ctx context.Context, req Request) ([]string, error) {
	var env []string
	for _, value := range os.Environ() {
		if !strings.HasPrefix(value, "CGO_ENABLED=") && !strings.HasPrefix(value, "GOWORK=") {
			env = append(env, value)
		}
	}
	env = append(env, "GOWORK=auto")
	if req.Cgo != "" {
		env = append(env, "CGO_ENABLED="+req.Cgo)
	}
	if req.Target != "" {
		parts := strings.Split(req.Target, "/")
		if len(parts) != 2 || parts[0] == "" || parts[1] == "" {
			return nil, fmt.Errorf("Go target must be an OS/architecture pair")
		}
		env = append(env, "GOOS="+parts[0], "GOARCH="+parts[1])
	}
	driver := exec.CommandContext(ctx, "go", "env", "-json", "GOOS", "GOARCH", "CGO_ENABLED", "GOVERSION", "GOWORK")
	driver.Dir, driver.Env = req.Dirs[0], env
	metadata, err := driver.Output()
	if err != nil {
		return nil, fmt.Errorf("Go environment: %w", err)
	}
	var effective map[string]string
	if err := json.Unmarshal(metadata, &effective); err != nil {
		return nil, err
	}
	b.graph.Profile["target"] = effective["GOOS"] + "/" + effective["GOARCH"]
	b.graph.Profile["cgo"] = effective["CGO_ENABLED"]
	b.graph.Profile["toolchain"] = effective["GOVERSION"]
	b.graph.Profile["workspace"] = effective["GOWORK"]
	return env, nil
}
func (b *builder) load(ctx context.Context, req Request, env []string) []*packages.Package {
	groups := req.Dirs
	_, workErr := os.Stat(filepath.Join(req.Root, "go.work"))
	if workErr == nil {
		groups = []string{req.Root}
	}
	var loaded []*packages.Package
	for _, dir := range groups {
		patterns := []string{"./..."}
		if workErr == nil {
			patterns = nil
			for _, module := range req.Dirs {
				patterns = append(patterns, filepath.Join(module, "..."))
			}
		}
		cfg := &packages.Config{Context: ctx, Mode: packages.NeedName | packages.NeedFiles | packages.NeedImports | packages.NeedDeps | packages.NeedModule, Dir: dir, Fset: b.fset, Tests: req.Tests, Env: env, BuildFlags: []string{"-mod=readonly"}}
		if len(req.Tags) > 0 {
			cfg.BuildFlags = append(cfg.BuildFlags, "-tags="+strings.Join(req.Tags, ","))
		}
		metadata, err := packages.Load(cfg, patterns...)
		if err != nil {
			b.fail(err.Error())
			continue
		}
		// Load typed syntax for local replacement dependencies too, not the entire stdlib.
		local := map[string]bool{}
		packages.Visit(metadata, nil, func(p *packages.Package) {
			if p.Module != nil && !filepath.IsAbs(b.path(p.Module.Dir)) && len(p.GoFiles) > 0 {
				local[p.PkgPath] = true
			}
		})
		for path := range local {
			patterns = append(patterns, path)
		}
		cfg.Mode = packages.LoadSyntax | packages.NeedModule | packages.NeedEmbedFiles
		pkgs, err := packages.Load(cfg, patterns...)
		if err != nil {
			b.fail(err.Error())
			continue
		}
		packages.Visit(pkgs, nil, func(p *packages.Package) {
			for _, err := range p.Errors {
				b.fail(err.Error())
			}
		})
		loaded = append(loaded, pkgs...)
	}
	return loaded
}
func moduleKey(p *packages.Package) string {
	if p.Module != nil {
		return p.Module.Path
	}
	return p.PkgPath
}
func (b *builder) rootsFor(p *packages.Package, req Request, library bool) {
	for _, u := range b.units {
		if u.pkg != p {
			continue
		}
		for _, id := range u.owners {
			decl := b.decls[id]
			if decl.Name == "main" && p.Name == "main" {
				b.seed(id, "main entry")
			}
			policyFile := decl.Span.File
			if !decl.Reportable {
				policyFile = b.path(b.fset.PositionFor(u.node.Pos(), true).Filename)
			}
			checkExports := req.CheckExports
			if check, ok := req.CheckExportFiles[policyFile]; ok {
				checkExports = check
			}
			if decl.Exported && library && !checkExports {
				b.seed(id, "public library API")
			}
			for _, file := range req.Entries {
				if policyFile == file {
					b.seed(id, "configured entry")
				}
			}
		}
	}
}
func (b *builder) collectPackages(loaded []*packages.Package, req Request) {
	mainModules := map[string]bool{}
	for _, p := range loaded {
		if p.Name == "main" && !strings.HasSuffix(p.PkgPath, ".test") {
			mainModules[moduleKey(p)] = true
		}
		if len(p.GoFiles) > 0 && p.TypesInfo != nil {
			b.collect(p)
			for _, instance := range p.TypesInfo.Instances {
				if n := named(instance.Type); n != nil {
					b.named = append(b.named, n)
				}
			}
		}
	}
	merged := map[string]Package{}
	for _, p := range loaded {
		pid := packageID(p)
		if b.decls[pid].ID == "" {
			continue
		}
		pkg := merged[pid]
		pkg.ID = pid
		for _, file := range append(append([]string{}, p.GoFiles...), p.EmbedFiles...) {
			pkg.Files = append(pkg.Files, b.path(file))
		}
		for path, dependency := range p.Imports {
			pkg.Imports = append(pkg.Imports, path)
			b.edge(pid, packageID(dependency), b.decls[pid].Span)
		}
		merged[pid] = pkg
		library := p.Name != "main" && !mainModules[moduleKey(p)]
		if p.Name == "main" || library {
			b.seed(pid, "package initialization")
		}
		b.rootsFor(p, req, library)
	}
	for _, p := range merged {
		p.Files = unique(p.Files)
		p.Imports = unique(p.Imports)
		b.graph.Packages = append(b.graph.Packages, p)
	}
	sort.Slice(b.graph.Packages, func(i, j int) bool { return b.graph.Packages[i].ID < b.graph.Packages[j].ID })
}
func unique(values []string) []string {
	set := map[string]bool{}
	for _, v := range values {
		set[v] = true
	}
	result := make([]string, 0, len(set))
	for v := range set {
		result = append(result, v)
	}
	sort.Strings(result)
	return result
}
func (b *builder) finish() {
	for _, d := range b.decls {
		b.graph.Declarations = append(b.graph.Declarations, d)
	}
	sort.Slice(b.graph.Declarations, func(i, j int) bool { return b.graph.Declarations[i].ID < b.graph.Declarations[j].ID })
	seen := map[Reference]bool{}
	refs := b.graph.References[:0]
	for _, r := range b.graph.References {
		if !seen[r] {
			refs = append(refs, r)
			seen[r] = true
		}
	}
	b.graph.References = refs
	sort.Slice(refs, func(i, j int) bool {
		a, c := refs[i], refs[j]
		if a.From != c.From {
			return a.From < c.From
		}
		if a.To != c.To {
			return a.To < c.To
		}
		if a.Span.File != c.Span.File {
			return a.Span.File < c.Span.File
		}
		if a.Span.Start != c.Span.Start {
			return a.Span.Start < c.Span.Start
		}
		return a.Span.End < c.Span.End
	})
	roots := b.graph.Roots[:0]
	seenRoots := map[Root]bool{}
	for _, r := range b.graph.Roots {
		if !seenRoots[r] {
			roots = append(roots, r)
			seenRoots[r] = true
		}
	}
	b.graph.Roots = roots
	sort.Slice(roots, func(i, j int) bool {
		if roots[i].Symbol != roots[j].Symbol {
			return roots[i].Symbol < roots[j].Symbol
		}
		return roots[i].Reason < roots[j].Reason
	})
}
func analyze(ctx context.Context, req Request) Graph {
	b := builder{root: req.Root, fset: token.NewFileSet(), decls: map[string]Declaration{}}
	b.graph = Graph{V: 1, Language: "Go", Complete: true, Profile: map[string]string{"toolchain": runtime.Version(), "tests": strconv.FormatBool(req.Tests), "tags": strings.Join(req.Tags, ",")},
		Declarations: []Declaration{}, References: []Reference{}, Roots: []Root{}, Packages: []Package{}, Diagnostics: []Diagnostic{}}
	if req.Root == "" || len(req.Dirs) == 0 {
		b.fail("Go analysis requires a root and module directories")
		return b.graph
	}
	env, err := b.environment(ctx, req)
	if err != nil {
		b.fail(err.Error())
		return b.graph
	}
	loaded := b.load(ctx, req, env)
	if !b.graph.Complete {
		return b.graph
	}
	if len(loaded) == 0 {
		b.fail("Go compiler selected no packages")
		return b.graph
	}
	b.collectPackages(loaded, req)
	b.references()
	b.finish()
	if len(b.graph.Roots) == 0 {
		b.fail("Go compiler selected no entry roots")
	}
	return b.graph
}
