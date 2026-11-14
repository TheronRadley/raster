# Raster command prompt

`raster --shell` opens a deliberately minimal prompt in an existing terminal;
there is no tab bar, status bar, or startup banner. The Windows installer makes
this the **Raster** Start menu shortcut and also provides **Raster Editor**.
Running `raster` with a filename or editor options still opens the editor, so
existing command-line workflows and files named `shell` continue to work.

Type `help` to list every Raster prompt command:

- `help` — show this list.
- `cd` — show the current directory; `cd path` changes it for later commands.
  On Windows `cd /d path` also works. Quote a path containing spaces if desired.
- `edit` — open the editor; `edit path` opens one literal file path (including
  spaces). `edit "path with spaces.txt"` works too. Closing the editor returns
  to the prompt. `edit` requires an interactive terminal.
- `exit` — close the prompt. End-of-input also closes it.

Other lines are executed by `cmd.exe /D /C` on Windows or `/bin/sh -c` on Unix,
with inherited standard input and output and the current directory. This gives
normal platform command syntax, pipes, redirection, and program execution. It
also means commands can modify or delete files: Raster does not sandbox them.
Use trusted commands only. Raster's `edit` and `cd` take a single literal path,
not shell expressions; invoke the platform shell explicitly for compound
operations involving them.

Each external command uses a **new** system-shell process. Only Raster's `cd`
persists between commands: `set`, `export`, aliases, shell functions, and other
changes in a child shell are not retained. Run `cmd` or your preferred shell
explicitly for a persistent full shell, then `exit` to return to Raster. Line
editing is provided by the host console; command history, completion, full
terminal emulation, tabs, and a graphical terminal host are not implemented.
`--shell` must be used alone; it does not change editor flags or file parsing.
When input is redirected, the prompt is omitted and `edit` is unavailable.
