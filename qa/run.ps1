# Thin Windows wrapper around qa/run.mjs (the runner itself is cross-platform).
#
#   .\qa\run.ps1                             # full matrix against front-end\dist
#   .\qa\run.ps1 -Scenarios empty,vcd,layout # a subset
#   .\qa\run.ps1 -Layout                     # R2 layout rules are required
#   .\qa\run.ps1 -Offline                    # self-hosted monaco check
#   .\qa\run.ps1 -List                       # print the scenario names
param(
  [string]$Scenarios = '',
  [switch]$Layout,
  [switch]$Offline,
  [string]$Dist = '',
  [switch]$Keep,
  [switch]$List
)

$ErrorActionPreference = 'Stop'
$runnerArgs = @()
if ($Scenarios) { $runnerArgs += @('--scenarios', $Scenarios) }
if ($Layout) { $runnerArgs += '--layout' }
if ($Offline) { $runnerArgs += '--offline' }
if ($Dist) { $runnerArgs += @('--dist', $Dist) }
if ($Keep) { $runnerArgs += '--keep' }
if ($List) { $runnerArgs += '--list' }

& node (Join-Path $PSScriptRoot 'run.mjs') @runnerArgs
exit $LASTEXITCODE
