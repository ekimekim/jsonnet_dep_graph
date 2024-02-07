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

#### Limitations

Does not support library paths as this makes import path resolution dependent on what files exist
(eg. `import "foo.jsonnet"` will first try `./foo.jsonnet`, then `LIB/foo.jsonnet`).

Uses jrsonnet 0.4.2 and would need a substantial rewrite to work with newer versions as the parser interface
does not appear to be stable.
In particular, this means that `importbin` is not supported and will trigger parse errors.
