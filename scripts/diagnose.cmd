@echo off
rem Read-only summary of a previously generated Raster health report.
rem Requires the .NET 10 SDK; arguments are passed to Raster.Diagnostics.
dotnet run --project "%~dp0..\tools\Raster.Diagnostics\Raster.Diagnostics.csproj" -- %*
exit /b %errorlevel%
