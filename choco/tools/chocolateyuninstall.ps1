$ErrorActionPreference = 'Stop'

$packageName = 'no-sleep-please'
$softwareName = 'No Sleep Please!*'

[array]$key = Get-UninstallRegistryKey -SoftwareName $softwareName

if ($key.Count -eq 1) {
  $file = "$($key[0].UninstallString)"
  Start-ChocolateyProcessAsAdmin "/S" "$file"
} elseif ($key.Count -eq 0) {
  Write-Warning "$packageName has already been uninstalled."
} elseif ($key.Count -gt 1) {
  Write-Warning "$($key.Count) matches found! Manual uninstall may be required."
  $key | ForEach-Object { Write-Warning "- $($_.DisplayName)" }
}
