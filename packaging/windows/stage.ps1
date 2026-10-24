# Copies the Windows zip's files, listed in zip-files.txt next to this
# script, to the top of -Dest. Zipping -Dest's contents gives the zip:
# ci.yml uploads -Dest as the windows-ui artifact and release.yml zips it.
param(
  [Parameter(Mandatory)] [string] $Dest,
  [Parameter(Mandatory)] [string] $Exe,
  [Parameter(Mandatory)] [string] $Ref,
  [Parameter(Mandatory)] [string] $Sha,
  [Parameter(Mandatory)] [string] $Repo
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$fill = [ordered]@{ '{{REF}}' = $Ref; '{{SHA}}' = $Sha; '{{REPO}}' = $Repo; '{{REPO_NAME}}' = $Repo.Split('/')[-1] }
if (Test-Path $Dest) { Remove-Item -Recurse -Force $Dest }
New-Item -ItemType Directory -Path $Dest | Out-Null
foreach ($line in Get-Content (Join-Path $PSScriptRoot 'zip-files.txt')) {
  $l = $line.Trim()
  if ($l -eq '' -or $l.StartsWith('#')) { continue }
  $f = @($l -split '\s+')
  if ($f.Count -ne 2) { throw "zip-files.txt: expected '<path in zip> <from>', got: $line" }
  $to = Join-Path $Dest $f[0]
  if ($f[1] -eq '$EXE') {
    Copy-Item -LiteralPath $Exe -Destination $to
  } elseif ($f[0].EndsWith('/')) {
    $from = Join-Path $root $f[1]
    if (Test-Path -LiteralPath $from -PathType Container) { $from = Join-Path $from '*' }
    $items = @(Get-Item -Path $from)
    if ($items.Count -eq 0) { throw "zip-files.txt: nothing matches $($f[1])" }
    New-Item -ItemType Directory -Force -Path $to | Out-Null
    Copy-Item -Path $items.FullName -Destination $to -Recurse
  } elseif ($f[1].EndsWith('.in')) {
    $text = [IO.File]::ReadAllText((Join-Path $root $f[1]))
    foreach ($k in $fill.Keys) { $text = $text.Replace($k, $fill[$k]) }
    if ($text.Contains('{{')) { throw "$($f[1]) has an unknown {{placeholder}}" }
    [IO.File]::WriteAllText($to, ($text -replace "`r?`n", "`r`n"))
  } else {
    Copy-Item -LiteralPath (Join-Path $root $f[1]) -Destination $to
  }
}
Get-ChildItem -Path $Dest -Recurse -File | ForEach-Object {
  [IO.Path]::GetRelativePath($Dest, $_.FullName).Replace('\', '/')
} | Sort-Object
