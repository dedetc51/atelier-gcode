param(
    [Parameter(Mandatory)][string]$Version,
    [Parameter(Mandatory)][string]$Binary,
    [Parameter(Mandatory)][string]$Output
)
$ErrorActionPreference = 'Stop'
if ($Version -notmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' -or
    ($Version.Split('.') | Where-Object { [long]$_ -gt 65535 })) {
    throw "Version Windows invalide ($Version)"
}
$compiler = Get-Command makensis.exe -ErrorAction SilentlyContinue
$makensis = if ($compiler) { $compiler.Source } else { Join-Path ${env:ProgramFiles(x86)} 'NSIS\makensis.exe' }
$exe = (Resolve-Path -LiteralPath $Binary).Path
$destination = [IO.Path]::GetFullPath($Output)
[IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($destination)) | Out-Null
& $makensis /V2 "/DVERSION=$Version" "/DBINARY=$exe" "/DOUTPUT=$destination" "$PSScriptRoot\installer.nsi"
if ($LASTEXITCODE -ne 0 -or !(Test-Path -LiteralPath $destination)) { throw "Échec de la création de l’installateur." }
