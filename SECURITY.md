# Security policy

## Reporting a vulnerability

Please report security issues through the repository's GitHub Security
Advisories ("Report a vulnerability") rather than a public issue. Include a
minimal reproduction, affected version/commit, and impact. Do not paste real
document contents, recovery files, or clipboard payloads into a public report.

This branch is version `0.1.0` and is not a finished release. Security fixes
land on the main development branch first. Hosted macOS/Windows/Termux
validation is still pending; see [docs/PLATFORMS.md](docs/PLATFORMS.md).

## Threat model and protections

Raster edits local files and may display untrusted filenames, file contents,
Git output, diagnostics, and language-server messages. The following behaviors
are intentional:

- **Terminal injection:** the renderer owns all ANSI/OSC escape sequences.
  Untrusted display text is sanitized; control characters and bidi overrides
  are replaced or escaped. File content is never emitted as raw terminal
  control bytes.
- **Bracketed paste:** paste sequences are decoded as text, not commands.
- **Document privacy:** session JSON stores paths and viewport metadata, not
  document contents. The panic hook suppresses panic payloads because they
  could contain document text. Normal logs must not contain file contents.
- **Recovery files:** local by default, inside a private state directory
  (`0700` on Unix) with private file permissions (`0600`). Windows ACL
  equivalence still needs CI validation.
- **Safe save:** atomic temp/flush/rename with a journal for interrupted saves;
  external modification detection; no silent dirty reload; permission and
  full-disk failures leave the original intact. LF/CRLF/mixed newlines and
  missing final newline are preserved; invalid UTF-8 and NUL bytes are refused.
- **Language servers:** disabled by default. Raster never installs/downloads a
  server, executes project scripts, or enables project-local executable
  configuration. Server commands come only from explicit user TOML config.
  Requests are versioned, stale responses are discarded, and hangs/crashes/
  excessive output are bounded. Basic editing continues if LSP fails.
- **WorkspaceEdits:** document versions and paths are validated; command
  actions are not executed automatically; broad/multi-file edits require user
  visibility and `trust_workspace_edits = false` by default.
- **Clipboard:** internal registers stay in-process. System clipboard and OSC
  52 are opt-in; OSC 52 payloads are capped; remote use requires
  `clipboard.allow_remote`. External clipboard programs are invoked without a
  shell.
- **Git:** read-only indicators only. No destructive Git commands.
- **Terminal restoration:** raw mode/cursor/paste/mouse/alt-screen state is
  restored on normal exit, recoverable errors, and panics where possible.
  Forced termination (`SIGKILL`, power loss) cannot run cleanup and is an
  explicit limitation.

## Known security-relevant limitations

- Full Arabic shaping/bidi rendering is not implemented; bidi controls are
  sanitized rather than interpreted. Visual spoofing through complex scripts
  remains possible. See [docs/LIMITATIONS.md](docs/LIMITATIONS.md).
- Windows console-mode save/restore, macOS signal handling, and Termux
  behavior are unverified locally; CI/device validation is pending.
- Real language servers and multi-file WorkspaceEdits have mock-server tests
  only so far.
- System clipboard behavior depends on external programs and the user's
  terminal; failures are reported but remote leakage is prevented only by the
  documented opt-ins.
- No signed release artifacts or distribution packages exist yet.
