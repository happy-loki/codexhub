[CmdletBinding()]
param(
    [ValidateRange(1024, 65535)]
    [int]$Port = 9335
)

$ErrorActionPreference = 'Stop'

if (Get-Process ChatGPT -ErrorAction SilentlyContinue) {
    throw 'Codex App is already running. This helper will not close or restart it. Exit Codex App first.'
}

$package = Get-AppxPackage OpenAI.Codex |
    Sort-Object Version -Descending |
    Select-Object -First 1
if (-not $package) {
    throw 'The OpenAI.Codex Store package is not installed.'
}

$source = @'
using System;
using System.Runtime.InteropServices;

[ComImport]
[Guid("2e941141-7f97-4756-ba1d-9decde894a3d")]
[InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IApplicationActivationManager
{
    int ActivateApplication(
        [MarshalAs(UnmanagedType.LPWStr)] string appUserModelId,
        [MarshalAs(UnmanagedType.LPWStr)] string arguments,
        uint options,
        out uint processId);

    int ActivateForFile(IntPtr appUserModelId, IntPtr itemArray, IntPtr verb, out uint processId);
    int ActivateForProtocol(IntPtr appUserModelId, IntPtr itemArray, out uint processId);
}

[ComImport]
[Guid("45BA127D-10A8-46EA-8AB7-56EA9078943C")]
class ApplicationActivationManager
{
}

public static class CodexHubMsixLauncher
{
    public static uint Launch(string appId, string arguments)
    {
        var manager = (IApplicationActivationManager)new ApplicationActivationManager();
        uint processId;
        int result = manager.ActivateApplication(appId, arguments, 0, out processId);
        Marshal.ThrowExceptionForHR(result);
        return processId;
    }
}
'@

if (-not ('CodexHubMsixLauncher' -as [type])) {
    Add-Type -TypeDefinition $source
}

$appUserModelId = "$($package.PackageFamilyName)!App"
$arguments = "--remote-debugging-address=127.0.0.1 --remote-debugging-port=$Port"
$processId = [CodexHubMsixLauncher]::Launch($appUserModelId, $arguments)

$deadline = (Get-Date).AddSeconds(30)
do {
    try {
        $targets = Invoke-RestMethod "http://127.0.0.1:$Port/json/list" -TimeoutSec 1
        if ($targets | Where-Object { $_.type -eq 'page' -and $_.url -like 'app://*' }) {
            Write-Output "Codex App PID $processId exposes loopback CDP on port $Port."
            exit 0
        }
    }
    catch {
    }
    Start-Sleep -Milliseconds 400
} while ((Get-Date) -lt $deadline)

throw "Codex App PID $processId did not expose an app:// renderer on port $Port within 30 seconds."
