$ErrorActionPreference = 'Stop'

$packageName = 'no-sleep-please'
$url64 = "https://github.com/shoootyou/non-sleep-please/releases/download/v$($env:ChocolateyPackageVersion)/No-Sleep-Please!_$($env:ChocolateyPackageVersion)_x64-setup.exe"

$packageArgs = @{
  packageName    = $packageName
  fileType       = 'exe'
  url64bit       = $url64
  silentArgs     = '/S'
  validExitCodes = @(0)
  softwareName   = 'No Sleep Please!*'
  checksumType64 = 'sha256'
  checksum64     = 'PLACEHOLDER'
}

Install-ChocolateyPackage @packageArgs
