//! Opt-in Go compiler adapter. The bundled helper runs outside the scanned tree.
use crate::{
    model::{Diagnostic, Graph, Language, ManifestKind, PackageUse, Project, Root},
    semantic::{SemanticGraph, SymbolId},
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Temporary(PathBuf);
impl Temporary {
    fn new() -> Result<Self, String> {
        for _ in 0..100 {
            let path = std::env::temp_dir().join(format!(
                "deadwood-go-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("cannot create private Go adapter directory".into())
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn target(project: &Project) -> String {
    project.config.target.clone().unwrap_or_else(|| {
        let os = std::env::var("GOOS").unwrap_or_else(|_| {
            match std::env::consts::OS {
                "macos" => "darwin",
                v => v,
            }
            .into()
        });
        let arch = std::env::var("GOARCH").unwrap_or_else(|_| {
            match std::env::consts::ARCH {
                "x86_64" => "amd64",
                "aarch64" => "arm64",
                "x86" => "386",
                v => v,
            }
            .into()
        });
        format!("{os}/{arch}")
    })
}
fn load(project: &Project) -> Result<SemanticGraph, String> {
    let dirs: Vec<_> = project
        .manifests()
        .filter(|m| m.kind == ManifestKind::Go)
        .filter_map(|m| m.path.parent())
        .collect();
    if dirs.is_empty() {
        return Err("--semantic-go requires a scanned Go module (go.mod/go.work)".into());
    }
    let temporary = Temporary::new()?;
    for (name, content) in [
        ("main.go", include_str!("../backends/go/main.go")),
        ("go.mod", include_str!("../backends/go/go.mod")),
        ("go.sum", include_str!("../backends/go/go.sum")),
    ] {
        fs::write(temporary.0.join(name), content).map_err(|e| e.to_string())?;
    }
    let entries: Vec<_> = crate::discover::roots(project)
        .0
        .into_iter()
        .filter(|r| {
            matches!(r.reason.as_str(), "configured entry" | "package entry")
                && project.files[r.file].language == Language::Go
        })
        .map(|r| &project.files[r.file].display_path)
        .collect();
    let timeout = Duration::from_secs(project.config.go_timeout.unwrap_or(120).clamp(1, 3600));
    let check_export_files: BTreeMap<_, _> = project
        .files
        .iter()
        .enumerate()
        .filter(|(_, f)| f.language == Language::Go)
        .map(|(i, f)| {
            (
                &f.display_path,
                crate::resolve::include_entry_exports(project, i),
            )
        })
        .collect();
    let input=serde_json::to_vec(&serde_json::json!({
        "root":project.root,"dirs":dirs,"tests":project.config.include_tests && !project.config.production,
        "tags":project.config.go_tags,"target":target(project),
        "check_exports":project.config.include_entry_exports,"check_export_files":check_export_files,"entries":entries,
        "timeout":timeout.as_secs(),"cgo":std::env::var("CGO_ENABLED").unwrap_or_default(),
    })).map_err(|e|e.to_string())?;
    let mut command = Command::new("go");
    command
        .args(["run", "-mod=readonly", "."])
        .current_dir(&temporary.0)
        .env("GOTOOLCHAIN", "local")
        .env("GOPACKAGESDRIVER", "off")
        .env("GOFLAGS", "")
        .env("GOWORK", "off")
        .env_remove("GOOS")
        .env_remove("GOARCH")
        .env("CGO_ENABLED", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !project.config.go_downloads {
        command.env("GOPROXY", "off").env("GOSUMDB", "off");
    }
    let result = execute(command, input, timeout)?;
    let status = result.status;
    let out = result.stdout;
    let err = result.stderr;
    if !status.success() {
        return Err(format!(
            "Go adapter failed: {}{}",
            String::from_utf8_lossy(&err).trim(),
            if project.config.go_downloads {
                ""
            } else {
                " (offline; use --go-downloads to permit fetching missing modules)"
            }
        ));
    }
    serde_json::from_slice(&out).map_err(|e| format!("invalid Go adapter output: {e}"))
}
fn terminate(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let _ = Command::new("/bin/kill")
            .args(["-KILL", "--", &format!("-{}", child.id())])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(windows)]
    {
        if let Some(root) = std::env::var_os("SystemRoot") {
            let _ = Command::new(PathBuf::from(root).join("System32/taskkill.exe"))
                .args(["/F", "/T", "/PID", &child.id().to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}
fn execute(
    mut command: Command,
    input: Vec<u8>,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    if input.len() > 1024 * 1024 {
        return Err("Go adapter input limit exceeded".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot start Go compiler: {e}"))?;
    let start = Instant::now();
    let out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let (out_tx, out_rx) = std::sync::mpsc::channel();
    let (err_tx, err_rx) = std::sync::mpsc::channel();
    let (in_tx, in_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = out_tx.send(read_bounded(out, 32 * 1024 * 1024));
    });
    std::thread::spawn(move || {
        let _ = err_tx.send(read_bounded(err, 1024 * 1024));
    });
    std::thread::spawn(move || {
        let _ = in_tx.send(stdin.write_all(&input));
    });
    let (mut status, mut output, mut error, mut written) = (None, None, None, None);
    loop {
        if status.is_none() {
            match child.try_wait() {
                Ok(s) => status = s,
                Err(e) => {
                    terminate(&mut child);
                    return Err(e.to_string());
                }
            }
        }
        if output.is_none() {
            output = out_rx.try_recv().ok()
        }
        if error.is_none() {
            error = err_rx.try_recv().ok()
        }
        if written.is_none() {
            written = in_rx.try_recv().ok()
        }
        if let (Some(status), Some(output), Some(error), Some(written)) =
            (status, output.as_mut(), error.as_mut(), written.as_ref())
        {
            written
                .as_ref()
                .map_err(|e| format!("Go adapter input failed: {e}"))?;
            return Ok(std::process::Output {
                status,
                stdout: std::mem::take(output.as_mut().map_err(|e| e.clone())?),
                stderr: std::mem::take(error.as_mut().map_err(|e| e.clone())?),
            });
        }
        if start.elapsed() >= timeout {
            terminate(&mut child);
            return Err("Go compiler analysis timed out".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn read_bounded(reader: impl Read, limit: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("Go adapter output limit exceeded".into());
    }
    Ok(bytes)
}
pub fn analyze(project: &Project, graph: &mut Graph) {
    let opaque_files: HashSet<_> = graph
        .edges
        .iter()
        .enumerate()
        .filter(|(i, _)| graph.reachable[*i] && project.files[*i].language != Language::Go)
        .flat_map(|(_, targets)| targets)
        .filter(|i| project.files[**i].language == Language::Go)
        .map(|i| project.files[*i].display_path.clone())
        .collect();
    let mut semantic = load(project).unwrap_or_else(|message| SemanticGraph {
        v: 1,
        language: Language::Go,
        complete: false,
        profile: BTreeMap::new(),
        declarations: Vec::new(),
        references: Vec::new(),
        roots: Vec::new(),
        packages: Vec::new(),
        diagnostics: vec![Diagnostic {
            kind: "semantic_error".into(),
            message,
        }],
    });
    // A Go source consumed as opaque data by another language is not dead code.
    for declaration in &semantic.declarations {
        if declaration.reportable && opaque_files.contains(&declaration.span.file) {
            semantic.roots.push(crate::semantic::SemanticRoot {
                symbol: declaration.id.clone(),
                reason: "opaque cross-language consumer".into(),
            });
        }
    }
    for package in &semantic.packages {
        if package.files.iter().any(|file| opaque_files.contains(file)) {
            semantic.roots.push(crate::semantic::SemanticRoot {
                symbol: package.id.clone(),
                reason: "opaque cross-language consumer".into(),
            });
        }
    }
    let live = semantic.reachable().and_then(|live| {
        if semantic.language != Language::Go {
            return Err("unexpected Go adapter language".into());
        }
        for declaration in &semantic.declarations {
            if declaration.reportable
                && let Some(file) = project
                    .files
                    .iter()
                    .find(|f| f.display_path == declaration.span.file)
                && (declaration.span.end > file.source.len()
                    || !file.source.is_char_boundary(declaration.span.start)
                    || !file.source.is_char_boundary(declaration.span.end))
            {
                return Err("Go declaration does not map to the analyzed source".into());
            }
        }
        if crate::cache::snapshot_with(&project.root, project.config.cache.as_deref())
            != project.stamp
        {
            return Err(
                "project changed during Go compiler analysis; rerun before trusting it".into(),
            );
        }
        if !semantic.packages.iter().any(|p| {
            p.files.iter().any(|path| {
                project
                    .files
                    .iter()
                    .any(|f| f.language == Language::Go && f.display_path == *path)
            })
        }) {
            return Err("Go compiler selected no scanned Go files".into());
        }
        Ok(live)
    });
    if let Err(message) = &live {
        semantic.complete = false;
        if semantic.diagnostics.is_empty() {
            semantic.diagnostics.push(Diagnostic {
                kind: "semantic_error".into(),
                message: message.clone(),
            });
        }
    }
    graph.diagnostics.extend(semantic.diagnostics.clone());
    // Never let heuristic Go findings masquerade as compiler-backed results.
    graph
        .package_uses
        .retain(|u| project.files[u.file].language != Language::Go);
    graph
        .unresolved
        .retain(|u| project.files[u.file].language != Language::Go);
    graph
        .used_symbols
        .retain(|(file, _)| project.files[*file].language != Language::Go);
    graph
        .roots
        .retain(|r| project.files[r.file].language != Language::Go);
    let go_paths: Vec<_> = project
        .files
        .iter()
        .filter(|f| f.language == Language::Go)
        .map(|f| &f.display_path)
        .collect();
    graph.diagnostics.retain(|d| {
        d.kind != "unresolved_reference"
            || !go_paths
                .iter()
                .any(|p| d.message.starts_with(&format!("{p}:")))
    });
    for (index, file) in project.files.iter().enumerate() {
        if file.language == Language::Go {
            graph.edges[index].clear();
            graph.reachable[index] = false;
        }
    }
    if let Ok(live) = live {
        apply(project, graph, &semantic, &live);
    }
    graph.semantic.push(semantic);
}
fn apply(project: &Project, graph: &mut Graph, semantic: &SemanticGraph, live: &HashSet<SymbolId>) {
    let indices: HashMap<_, _> = project
        .files
        .iter()
        .enumerate()
        .map(|(i, f)| (&f.display_path, i))
        .collect();
    let package_files: HashMap<_, Vec<_>> = semantic
        .packages
        .iter()
        .map(|p| {
            (
                &p.id,
                p.files
                    .iter()
                    .filter_map(|f| indices.get(f).copied())
                    .collect(),
            )
        })
        .collect();
    for package in &semantic.packages {
        let files: Vec<_> = package
            .files
            .iter()
            .filter_map(|f| indices.get(f).copied())
            .collect();
        let mut targets = files.clone();
        for import in &package.imports {
            if let Some(imported) = package_files.get(&SymbolId(format!("package:{import}"))) {
                targets.extend(imported);
            }
        }
        targets.sort_unstable();
        targets.dedup();
        for &file in &files {
            if project.files[file].language == Language::Go {
                graph.edges[file] = targets.clone();
            }
            if live.contains(&package.id) {
                graph.reachable[file] = true;
            }
            if !live.contains(&package.id) || project.files[file].language != Language::Go {
                continue;
            }
            for import in &project.files[file].imports {
                if package.imports.contains(&import.raw) {
                    graph.package_uses.push(PackageUse {
                        file,
                        line: import.line,
                        name: import.raw.clone(),
                        raw: import.raw.clone(),
                        resolved: package_files
                            .contains_key(&SymbolId(format!("package:{}", import.raw))),
                    });
                }
            }
        }
        if live.contains(&package.id)
            && let Some(&file) = files.first()
        {
            graph.roots.push(Root {
                file,
                reason: "Go compiler package".into(),
            });
        }
    }
    graph
        .diagnostics
        .retain(|d| !matches!(d.kind.as_str(), "no_entrypoints" | "no_source_files"));
    graph
        .package_uses
        .sort_by(|a, b| a.file.cmp(&b.file).then(a.raw.cmp(&b.raw)));
    graph
        .package_uses
        .dedup_by(|a, b| a.file == b.file && a.raw == b.raw);
    graph.used_packages = graph.package_uses.iter().map(|u| u.name.clone()).collect();
    graph.used_modules = graph.package_uses.iter().map(|u| u.raw.clone()).collect();
}
