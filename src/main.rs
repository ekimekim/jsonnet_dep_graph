use jrsonnet_parser::*;
use std::path::{Path, PathBuf};
use std::collections::{HashMap, HashSet};
use std::collections::hash_map::Entry;

struct Resolver<'a> {
	base_dir: &'a Path,
	jpaths: &'a [&'a Path],
}

impl<'a> Resolver<'a> {
	fn resolve(&self, path: &Path) -> Result<PathBuf, String> {
		// If path is absolute, no need to check anything either as the prefix doesn't matter.
		if path.is_absolute() {
			return Ok(path.to_owned());
		}
		// If no jpaths set, this is a no-op and doesn't need to check for existence.
		if self.jpaths.is_empty() {
			return Ok(self.base_dir.join(path));
		}
		// Find the first extant match.
		// Fail if we can't determine existence for any candidate.
		for prefix in std::iter::once(self.base_dir).chain(self.jpaths.iter().copied()) {
			let candidate = prefix.join(path);
			let exists = candidate.try_exists().map_err(|e|
				format!("Could not check path {}: {}", path.display(), e)
			)?;
			if exists {
				return Ok(candidate);
			}
		}
		// None existed, fall back to the local case.
		// This seems more useful than erroring.
		// It will likely error later anyway, when we try to parse that file.
		// However, this behaviour is useful if the subject is a leaf dep
		// that is a generated file.
		return Ok(self.base_dir.join(path));
	}
}

#[derive(Default, Debug)]
struct Analysis {
	// Leaf deps are static files, where only a change in the file itself
	// can affect the analysed file.
	leaf_deps: Vec<PathBuf>,
	// Deep deps are jsonnet files where a change in that file *or any of its dependences*
	// can affect the analysed file.
	deep_deps: Vec<PathBuf>,
}

fn analyze_file(jpaths: &[&Path], filepath: &Path) -> Result<Analysis, String> {
	let contents = std::fs::read_to_string(filepath).map_err(|e|
		format!("Failed to read {}: {}", filepath.display(), e)
	)?;

	let settings = ParserSettings {
		loc_data: false,
		file_name: filepath.to_owned().into(),
	};

	let ast = parse(&contents, &settings).map_err(|e|
		format!("Failed to parse {}: {}", filepath.display(), e)
	)?;

	// Path should always have a parent given we managed to open it as a file earlier, so it
	// can't be a directory or "".
	let base_dir = filepath.parent().unwrap();
	let resolver = Resolver { base_dir, jpaths };

	let mut analysis = Analysis::default();
	scan_ast(&resolver, &mut analysis, &ast)?;

	Ok(analysis)
}

fn add_path(resolver: &Resolver, paths: &mut Vec<PathBuf>, path: &Path) -> Result<(), String> {
	let path = resolver.resolve(path)?;
	if !paths.contains(&path) {
		paths.push(path);
	}
	Ok(())
}

fn scan_ast(resolver: &Resolver, analysis: &mut Analysis, expr: &LocExpr) -> Result<(), String> {
	match &*expr.0 {
		// Base cases: We found actual imports!
		Expr::Import(path) => add_path(resolver, &mut analysis.deep_deps, path)?,
		Expr::ImportStr(path) => add_path(resolver, &mut analysis.leaf_deps, &path)?,
		// Otherwise, recurse if needed
		Expr::Arr(exprs) => for expr in exprs { scan_ast(resolver, analysis, expr)? },
		Expr::ArrComp(expr, compspecs) => {
			scan_ast(resolver, analysis, expr)?;
			scan_compspecs(resolver, analysis, compspecs)?;
		},
		Expr::Obj(obj) => scan_obj(resolver, analysis, obj)?,
		Expr::ObjExtend(expr, obj) => {
			scan_ast(resolver, analysis, expr)?;
			scan_obj(resolver, analysis, obj)?;
		},
		Expr::Parened(expr) => scan_ast(resolver, analysis, expr)?,
		Expr::UnaryOp(_, expr) => scan_ast(resolver, analysis, expr)?,
		Expr::BinaryOp(expr_a, _, expr_b) => {
			scan_ast(resolver, analysis, expr_a)?;
			scan_ast(resolver, analysis, expr_b)?;
		},
		Expr::AssertExpr(AssertStmt(expr_a, maybe_expr_b), expr_c) => {
			scan_ast(resolver, analysis, expr_a)?;
			if let Some(expr) = maybe_expr_b {
				scan_ast(resolver, analysis, expr)?;
			}
			scan_ast(resolver, analysis, expr_c)?;
		},
		Expr::LocalExpr(bindspecs, expr) => {
			for bindspec in bindspecs {
				scan_bindspec(resolver, analysis, bindspec)?;
			}
			scan_ast(resolver, analysis, expr)?;
		},
		Expr::ErrorStmt(expr) => scan_ast(resolver, analysis, expr)?,
		Expr::Apply(expr, args, _) => {
			scan_ast(resolver, analysis, expr)?;
			for Arg(_, expr) in &args.0 {
				scan_ast(resolver, analysis, expr)?;
			}
		},
		Expr::Index(expr_a, expr_b) => {
			scan_ast(resolver, analysis, expr_a)?;
			scan_ast(resolver, analysis, expr_b)?;
		},
		Expr::Function(params, expr) => {
			for Param(_, maybe_expr) in &*params.0 {
				if let Some(expr) = maybe_expr {
					scan_ast(resolver, analysis, expr)?;
				}
			}
			scan_ast(resolver, analysis, expr)?;
		},
		Expr::IfElse{cond, cond_then, cond_else} => {
			scan_ast(resolver, analysis, &cond.0)?;
			scan_ast(resolver, analysis, cond_then)?;
			if let Some(expr) = cond_else {
				scan_ast(resolver, analysis, expr)?;
			}
		},
		Expr::Slice(expr, SliceDesc{start, end, step}) => {
			scan_ast(resolver, analysis, expr)?;
			for maybe_expr in [start, end, step] {
				if let Some(expr) = maybe_expr {
					scan_ast(resolver, analysis, expr)?;
				}
			}
		},
		// Remaining cases are leaf nodes like literals that we don't care about.
		_ => (),
	}
	Ok(())
}

fn scan_compspecs(resolver: &Resolver, analysis: &mut Analysis, compspecs: &[CompSpec]) -> Result<(), String> {
	for compspec in compspecs {
		match compspec {
			CompSpec::IfSpec(data) => scan_ast(resolver, analysis, &data.0)?,
			CompSpec::ForSpec(data) => scan_ast(resolver, analysis, &data.1)?,
		}
	}
	Ok(())
}

fn scan_bindspec(resolver: &Resolver, analysis: &mut Analysis, bindspec: &BindSpec) -> Result<(), String> {
	let BindSpec{params, value, ..} = bindspec;
	if let Some(params) = params {
		for Param(_, maybe_expr) in &*params.0 {
			if let Some(expr) = maybe_expr {
				scan_ast(resolver, analysis, expr)?;
			}
		}
	}
	scan_ast(resolver, analysis, value)?;
	Ok(())
}

fn scan_obj(resolver: &Resolver, analysis: &mut Analysis, obj: &ObjBody) -> Result<(), String> {
	match obj {
		ObjBody::MemberList(members) => {
			for member in members {
				match member {
					Member::Field(FieldMember{name, params, value, ..}) => {
						match name {
							FieldName::Fixed(_) => (),
							FieldName::Dyn(expr) => scan_ast(resolver, analysis, expr)?,
						}
						if let Some(params) = params {
							for Param(_, maybe_expr) in &*params.0 {
								if let Some(expr) = maybe_expr {
									scan_ast(resolver, analysis, expr)?;
								}
							}
						}
						scan_ast(resolver, analysis, value)?;
					},
					Member::BindStmt(bindspec) => scan_bindspec(resolver, analysis, bindspec)?,
					Member::AssertStmt(AssertStmt(expr, maybe_expr)) => {
						scan_ast(resolver, analysis, expr)?;
						if let Some(expr) = maybe_expr {
							scan_ast(resolver, analysis, expr)?;
						}
					},
				}
			}
		},
		ObjBody::ObjComp(ObjComp{pre_locals, key, value, post_locals, compspecs}) => {
			for bindspec in pre_locals { scan_bindspec(resolver, analysis, bindspec)?; }
			scan_ast(resolver, analysis, key)?;
			scan_ast(resolver, analysis, value)?;
			for bindspec in post_locals { scan_bindspec(resolver, analysis, bindspec)?; }
			scan_compspecs(resolver, analysis, compspecs)?;
		},
	}
	Ok(())
}

fn resolve_deps(cache: &mut HashMap<PathBuf, Analysis>, jpaths: &[&Path], filename: &Path) -> Result<HashSet<PathBuf>, String> {
	let mut deps: HashSet<PathBuf> = HashSet::new();
	let mut to_expand = vec![filename.to_owned()];
	while let Some(filename) = to_expand.pop() {
		// It's possible to have already seen this dep, if the dependency graph contains loops.
		// In that case, don't expand to avoid infinite looping.
		if deps.contains(&filename) {
			continue;
		}
		deps.insert(filename.clone());
		// We can't just use or_insert_with() because analyse_file may error,
		// so we need to do it the long way.
		let analysis = match cache.entry(filename) {
			Entry::Occupied(entry) => entry.into_mut(),
			Entry::Vacant(entry) => {
				let analysis = analyze_file(jpaths, entry.key())?;
				entry.insert(analysis)
			}
		};
		// leaf deps can be added immediately to the full set, and don't need to be expanded.
		for leaf_dep in &analysis.leaf_deps {
			deps.insert(leaf_dep.clone());
		}
		// deep deps go into the expand list.
		for deep_dep in &analysis.deep_deps {
			to_expand.push(deep_dep.clone());
		}
	}
	Ok(deps)
}

fn inner_main() -> Result<(), String> {
	// Argument parsing
	let mut files: Vec<PathBuf> = Vec::new();
	let mut jpaths: Vec<PathBuf> = Vec::new();
	let mut args = std::env::args();
	let progname = args.next().ok_or("Missing arg 0")?;
	while let Some(arg) = args.next() {
		match arg.as_str() {
			"--help" => return Err(format!("Usage: {} {{FILENAME | --jpath PATH}}", progname)),
			"--jpath" => {
				let path = args.next().ok_or("Missing argument to --jpath")?;
				jpaths.push(path.into());
			},
			filepath => files.push(filepath.into()),
		}
	}

	let mut cache: HashMap<PathBuf, Analysis> = HashMap::new();
	let jpaths: Vec<&Path> = jpaths.iter().map(|path| path.as_path()).collect();
	for filepath in files {
		let deps = resolve_deps(&mut cache, &jpaths, &filepath)?;
		let as_str: Vec<_> = deps.iter().map(|p| p.to_string_lossy()).collect();
		println!("{}: {}", filepath.display(), as_str.join(" "));
	}
	Ok(())
}

fn main() -> std::process::ExitCode {
	match inner_main() {
		Ok(()) => 0,
		Err(e) => {
			eprintln!("{}", e);
			1
		}
	}.into()
}
