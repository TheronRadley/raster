using System.Text.Json;

// A read-only companion for Raster's --health output. No editor data or paths
// are displayed, and no subprocesses are launched.
const int maxBytes = 1024 * 1024;
if (args.Length != 1 || args[0] is "--help" or "-h")
{
    Console.Error.WriteLine("Usage: Raster.Diagnostics <health.json|->");
    Console.Error.WriteLine("Generate a report with: raster --health > health.json");
    return args.Length == 1 ? 0 : 2;
}

try
{
    using Stream source = args[0] == "-" ? Console.OpenStandardInput() : File.OpenRead(args[0]);
    using var bytes = new MemoryStream();
    var buffer = new byte[8192];
    int count;
    while ((count = source.Read(buffer, 0, buffer.Length)) != 0)
    {
        if (bytes.Length + count > maxBytes)
        {
            Console.Error.WriteLine("raster-diagnostics: report exceeds 1 MiB");
            return 2;
        }
        bytes.Write(buffer, 0, count);
    }
    bytes.Position = 0;
    using var document = JsonDocument.Parse(bytes);
    var root = document.RootElement;
    if (root.ValueKind != JsonValueKind.Object ||
        !root.TryGetProperty("status", out var status) || status.ValueKind != JsonValueKind.String ||
        !root.TryGetProperty("config_valid", out var valid) ||
        (valid.ValueKind != JsonValueKind.True && valid.ValueKind != JsonValueKind.False) ||
        !root.TryGetProperty("stdin_tty", out var stdin) ||
        (stdin.ValueKind != JsonValueKind.True && stdin.ValueKind != JsonValueKind.False) ||
        !root.TryGetProperty("stdout_tty", out var stdout) ||
        (stdout.ValueKind != JsonValueKind.True && stdout.ValueKind != JsonValueKind.False))
    {
        Console.Error.WriteLine("raster-diagnostics: invalid health report schema");
        return 2;
    }

    bool healthy = status.GetString() == "ok" && valid.GetBoolean();
    Console.WriteLine(healthy ? "Raster health: OK" : "Raster health: needs attention");
    Console.WriteLine(valid.GetBoolean() ? "Configuration: valid" : "Configuration: invalid");
    Console.WriteLine($"Terminal input: {(stdin.GetBoolean() ? "interactive" : "redirected")}");
    Console.WriteLine($"Terminal output: {(stdout.GetBoolean() ? "interactive" : "redirected")}");
    Console.WriteLine("Note: redirected streams are expected when generating a report in a pipeline.");
    Console.WriteLine("Paths and document contents are not displayed.");
    return healthy ? 0 : 1;
}
catch (Exception error) when (error is IOException or UnauthorizedAccessException or JsonException or ArgumentException or NotSupportedException)
{
    // Avoid printing input paths or report contents, which can be sensitive.
    Console.Error.WriteLine("raster-diagnostics: cannot read a valid health report");
    return 2;
}
