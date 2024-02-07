use jrsonnet_parser::*;
use std::path::{Path, PathBuf};
use std::collections::{HashMap, HashSet};
use std::collections::hash_map::Entry;

#[derive(Default, Debug)]
struct Analysis {
	// Leaf deps are static files, where only a change in the file itself
	// can affect the analysed file.
	leaf_deps: Vec<PathBuf>,
	// Deep deps are jsonnet files where a change in that file *or any of its dependences*
	// can affect the analysed file.
	deep_deps: Vec<PathBuf>,
}

fn analyze_file(filepath: &Path) -> Result<Analysis, String> {
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

	let mut analysis = Analysis::default();

	scan_ast(&mut analysis, &ast);

	Ok(analysis)
}

fn scan_ast(analysis: &mut Analysis, expr: &LocExpr) {
	match &*expr.0 {
		// Base cases: We found actual imports!
		Expr::Import(path) => analysis.deep_deps.push(path.clone()),
		Expr::ImportStr(path) => analysis.leaf_deps.push(path.clone()),
		// Otherwise, recurse if needed
		Expr::Arr(exprs) => for expr in exprs { scan_ast(analysis, expr) },
		Expr::ArrComp(expr, compspecs) => {
			scan_ast(analysis, expr);
			scan_compspecs(analysis, compspecs);
		},
		Expr::Obj(obj) => scan_obj(analysis, obj),
		Expr::ObjExtend(expr, obj) => {
			scan_ast(analysis, expr);
			scan_obj(analysis, obj);
		},
		Expr::Parened(expr) => scan_ast(analysis, expr),
		Expr::UnaryOp(_, expr) => scan_ast(analysis, expr),
		Expr::BinaryOp(expr_a, _, expr_b) => {
			scan_ast(analysis, expr_a);
			scan_ast(analysis, expr_b);
		},
		Expr::AssertExpr(AssertStmt(expr_a, maybe_expr_b), expr_c) => {
			scan_ast(analysis, expr_a);
			if let Some(expr) = maybe_expr_b {
				scan_ast(analysis, expr);
			}
			scan_ast(analysis, expr_c);
		},
		Expr::LocalExpr(bindspecs, expr) => {
			for bindspec in bindspecs {
				scan_bindspec(analysis, bindspec);
			}
			scan_ast(analysis, expr);
		},
		Expr::ErrorStmt(expr) => scan_ast(analysis, expr),
		Expr::Apply(expr, args, _) => {
			scan_ast(analysis, expr);
			for Arg(_, expr) in &args.0 {
				scan_ast(analysis, expr);
			}
		},
		Expr::Index(expr_a, expr_b) => {
			scan_ast(analysis, expr_a);
			scan_ast(analysis, expr_b);
		},
		Expr::Function(params, expr) => {
			for Param(_, maybe_expr) in &*params.0 {
				if let Some(expr) = maybe_expr {
					scan_ast(analysis, expr);
				}
			}
			scan_ast(analysis, expr);
		},
		Expr::IfElse{cond, cond_then, cond_else} => {
			scan_ast(analysis, &cond.0);
			scan_ast(analysis, cond_then);
			if let Some(expr) = cond_else {
				scan_ast(analysis, expr);
			}
		},
		Expr::Slice(expr, SliceDesc{start, end, step}) => {
			scan_ast(analysis, expr);
			for maybe_expr in [start, end, step] {
				if let Some(expr) = maybe_expr {
					scan_ast(analysis, expr);
				}
			}
		},
		// Remaining cases are leaf nodes like literals that we don't care about.
		_ => (),
	}
}

fn scan_compspecs(analysis: &mut Analysis, compspecs: &[CompSpec]) {
	for compspec in compspecs {
		match compspec {
			CompSpec::IfSpec(data) => scan_ast(analysis, &data.0),
			CompSpec::ForSpec(data) => scan_ast(analysis, &data.1),
		}
	}
}

fn scan_bindspec(analysis: &mut Analysis, bindspec: &BindSpec) {
	let BindSpec{params, value, ..} = bindspec;
	if let Some(params) = params {
		for Param(_, maybe_expr) in &*params.0 {
			if let Some(expr) = maybe_expr {
				scan_ast(analysis, expr);
			}
		}
	}
	scan_ast(analysis, value);
}

fn scan_obj(analysis: &mut Analysis, obj: &ObjBody) {
	match obj {
		ObjBody::MemberList(members) => {
			for member in members {
				match member {
					Member::Field(FieldMember{name, params, value, ..}) => {
						match name {
							FieldName::Fixed(_) => (),
							FieldName::Dyn(expr) => scan_ast(analysis, expr),
						}
						if let Some(params) = params {
							for Param(_, maybe_expr) in &*params.0 {
								if let Some(expr) = maybe_expr {
									scan_ast(analysis, expr);
								}
							}
						}
						scan_ast(analysis, value);
					},
					Member::BindStmt(bindspec) => scan_bindspec(analysis, bindspec),
					Member::AssertStmt(AssertStmt(expr, maybe_expr)) => {
						scan_ast(analysis, expr);
						if let Some(expr) = maybe_expr {
							scan_ast(analysis, expr);
						}
					},
				}
			}
		},
		ObjBody::ObjComp(ObjComp{pre_locals, key, value, post_locals, compspecs}) => {
			for bindspec in pre_locals { scan_bindspec(analysis, bindspec); }
			scan_ast(analysis, key);
			scan_ast(analysis, value);
			for bindspec in post_locals { scan_bindspec(analysis, bindspec); }
			scan_compspecs(analysis, compspecs);
		},
	}
}

fn resolve_deps(cache: &mut HashMap<PathBuf, Analysis>, filename: &Path) -> Result<HashSet<PathBuf>, String> {
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
				let analysis = analyze_file(entry.key())?;
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

fn main() -> Result<(), String> {
	let args: Vec<_> = std::env::args().skip(1).collect();
	let mut cache: HashMap<PathBuf, Analysis> = HashMap::new();
	for arg in &args {
		let deps = resolve_deps(&mut cache, Path::new(arg))?;
		println!("{}: {:?}", arg, deps);
	}
	Ok(())
}
