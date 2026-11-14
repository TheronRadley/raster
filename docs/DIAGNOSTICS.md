# Optional C# health-report companion

Raster remains a cross-platform terminal editor running inside an existing
terminal. Inspired by Windows Terminal's attention to terminal compatibility
and platform-specific tools, this optional companion summarizes the existing
`raster --health` JSON; it does not embed or copy Windows Terminal code, modify
Raster configuration, inspect files, launch the editor, or replace its renderer.

## Usage

Requires the .NET 10 SDK. No external NuGet packages are referenced.

```sh
cargo run --offline --locked --bin raster -- --health > health.json
dotnet run --project tools/Raster.Diagnostics -- health.json
# Or read a report from standard input:
cat health.json | dotnet run --project tools/Raster.Diagnostics -- -
```

On Windows, `scripts\diagnose.cmd health.json` provides the same entry point.
Quote file paths containing spaces. To build and test the companion directly:

```sh
dotnet build tools/Raster.Diagnostics/Raster.Diagnostics.csproj -c Release
python3 scripts/test_diagnostics.py tools/Raster.Diagnostics/bin/Release/net10.0/Raster.Diagnostics.dll health.json
```

The summary deliberately omits paths and document contents. **The original
health JSON still contains paths**; protect it and remove it when no longer
needed. A report generated with redirected output or in a pipeline naturally
reports redirected streams. That alone does not mean a terminal is broken;
this tool checks report fields, not actual interactive rendering capabilities.
Exit codes are 0 for an OK, valid-config report, 1 for an unhealthy report,
and 2 for usage, input, or schema errors. Input is limited to 1 MiB.

The CI workflow builds and tests the C# tool and batch wrapper on Windows.
They are optional; ordinary editor builds do not require .NET. CI does not
validate Windows interactive terminal behavior.

## Rendering scope

Windows Terminal uses a graphical terminal-host architecture, including GPU
rendering. Raster emits terminal sequences to an existing host and has no
DirectX, GPU swapchain, or shader-loading pipeline. There is therefore no HLSL
shader to compile or run in Raster today. A shader added without such a backend
would be dead code; graphical rendering remains a separate architectural
choice, not a feature implied by this diagnostics tool.
