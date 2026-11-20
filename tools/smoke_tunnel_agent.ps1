# Windows: smoke_tunnel_agent.ps1 <rust-remote.exe> <rathole.exe>
param([Parameter(Mandatory)][string]$WorkspaceExe, [Parameter(Mandatory)][string]$RatholeExe)
$ErrorActionPreference = 'Stop'
$workspace = (Resolve-Path -LiteralPath $WorkspaceExe).Path
$rathole = (Resolve-Path -LiteralPath $RatholeExe).Path
$dir = New-Item -ItemType Directory -Path (Join-Path ([IO.Path]::GetTempPath()) ("rrw-agent-test-" + [guid]::NewGuid()))
$config = Join-Path $dir.FullName client.toml
$owner = [Security.Principal.WindowsIdentity]::GetCurrent().User
$secret = [guid]::NewGuid().ToString('N')
$agent = $null
$child = $null

function Set-TestAcl([bool]$allowOthers) {
    $acl = [Security.AccessControl.FileSecurity]::new()
    $acl.SetAccessRuleProtection($true, $false)
    $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($owner, 'FullControl', 'Allow'))
    if ($allowOthers) {
        $everyone = [Security.Principal.SecurityIdentifier]::new('S-1-1-0')
        $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($everyone, 'Read', 'Allow'))
    }
    # Change only the test file's DACL; auditing and ownership need no modification.
    [IO.FileSystemAclExtensions]::SetAccessControl([IO.FileInfo]::new($config), $acl)
}
function Start-Agent {
    $options = [Diagnostics.ProcessStartInfo]::new($workspace)
    $options.UseShellExecute = $false
    $options.CreateNoWindow = $true
    $options.RedirectStandardInput = $true
    $options.RedirectStandardError = $true
    foreach ($arg in @('tunnel-agent','--config',$config,'--rathole',$rathole,'--stop-on-stdin-close')) {
        [void]$options.ArgumentList.Add($arg)
    }
    [Diagnostics.Process]::Start($options)
}
function Read-Event($process) {
    $read = $process.StandardError.ReadLineAsync()
    if (!$read.Wait(8000)) { throw 'Agent status timeout.' }
    if ($read.Result -like "*$secret*") { throw 'Private content leaked in agent status.' }
    return $read.Result
}
function Get-RatholeChild($process) {
    $line = Read-Event $process
    if ($line -notmatch '^agent: running \(pid (\d+)\)$') { throw 'Missing child status.' }
    $proc = Get-Process -Id ([int]$Matches[1])
    $parent = (Get-CimInstance Win32_Process -Filter "ProcessId=$($proc.Id)").ParentProcessId
    if ($parent -ne $process.Id -or $proc.Path -ne $rathole) { throw 'Child identity did not match the test executable.' }
    return $proc
}
try {
    # The permissive-file negative test contains no credential.
    [IO.File]::WriteAllText($config, '[client]')
    Set-TestAcl $true
    $agent = Start-Agent
    if (!$agent.WaitForExit(8000) -or $agent.ExitCode -eq 0 -or $agent.StandardError.ReadToEnd() -notlike '*private client configuration*') {
        throw 'Broad ACL was not rejected before configuration parsing.'
    }
    Set-TestAcl $false
    [IO.File]::WriteAllText($config, "token = '$secret") # Deliberately invalid TOML
    $agent = Start-Agent
    if (!$agent.WaitForExit(8000) -or $agent.ExitCode -eq 0 -or $agent.StandardError.ReadToEnd().Contains($secret)) {
        throw 'Unsafe parser diagnostics or missing error.'
    }
    # Hold an unlistened loopback port so the official client cannot contact another service.
    $socket = [Net.Sockets.TcpClient]::new()
    $socket.Client.Bind([Net.IPEndPoint]::new([Net.IPAddress]::Loopback, 0))
    $port = $socket.Client.LocalEndPoint.Port
    [IO.File]::WriteAllText($config, "[client]`nremote_addr='127.0.0.1:$port'`n[client.services.ssh]`nlocal_addr='127.0.0.1:$port'`ntoken='$secret'`n")
    $agent = Start-Agent
    $child = Get-RatholeChild $agent
    $child.Kill()
    if ((Read-Event $agent) -notlike 'agent: retry in 1s*') { throw 'Missing first retry.' }
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $child = Get-RatholeChild $agent
    if ($timer.ElapsedMilliseconds -lt 900) { throw 'Child relaunched without backoff.' }
    $agent.StandardInput.Close()
    if (!(($agent.WaitForExit(8000)) -and $agent.ExitCode -eq 0 -and $child.WaitForExit(2000))) { throw 'Stop left a running child.' }
    $agent = Start-Agent
    $child = Get-RatholeChild $agent
    $child.Kill()
    if ((Read-Event $agent) -notlike 'agent: retry in 1s*') { throw 'No retry state.' }
    $agent.StandardInput.Close()
    if (!$agent.WaitForExit(8000) -or $agent.ExitCode -ne 0 -or $agent.StandardError.ReadToEnd().Trim() -ne 'agent: stopped') { throw 'Stop during backoff relaunched a child.' }
    'PASS: private ACL, redacted parse failure, real child restart with backoff, stop/reap, and stop during backoff.'
} finally {
    if ($agent -and !$agent.HasExited) { $agent.StandardInput.Close(); if (!$agent.WaitForExit(3000)) { $agent.Kill() } }
    if ($child -and !$child.HasExited) { $child.Kill(); $child.WaitForExit() }
    if ($socket) { $socket.Dispose() }
    if (Test-Path -LiteralPath $config) { Remove-Item -LiteralPath $config }
    $dir.Delete()
}
