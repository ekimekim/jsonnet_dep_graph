Uses [jrsonnet](https://github.com/CertainLach/jrsonnet)'s parser to read jsonnet files
and output all other files they import, as well as all files those files import, etc.

The intent is that this can be used to build a dependency graph of files so you only need
to rebuild if one of the files you depend on changes.

#### Usage

Run with any number of arguments, each one is treated as a top-level jsonnet file
to analyze dependencies of.

It will output one line per argument, in the form:
```
FILE: DEP DEP DEP
```
Note that FILE is included in the list of deps.

#### Library paths

This has basic support for library paths (`--jpath` on the `jsonnet` CLI), but it changes the behaviour
somewhat as to disambiguate whether `import "foo.jsonnet"` refers to a relative file or a library path.

Specify library paths by providing one or more `--jpath PATH` arguments.
If at least one is given, then for each relative import, the following will be searched in order:
- The directory of the file the import is in
- Each library path, in the order given

and uses the first path where that file currently exists.

If any of the paths have an error besides "does not exist" (for example, a permission error),
the whole process will fail.

If the file does not exist anywhere, the directory of the file is assumed, but note that in most
cases this will still lead to failure as we also need to read this file for further imports.

The one case where a non-existent file won't cause problems is if it is imported via `importstr`
and so doesn't need to be examined further. This may be useful in cases where such files are
generated later.

#### Limitations

Uses jrsonnet 0.4.2 and would need a substantial rewrite to work with newer versions as the parser interface
does not appear to be stable.
In particular, this means that `importbin` is not supported and will trigger parse errors.
