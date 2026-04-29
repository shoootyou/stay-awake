$ErrorActionPreference = 'Stop'

$packageName = 'stay-awake'
$url64 = "https://github.com/shoootyou/stay-awake/releases/download/v$($env:ChocolateyPackageVersion)/Stay.Awake_$($env:ChocolateyPackageVersion)_x64-setup.exe"

$packageArgs = @{
  packageName    = $packageName
  fileType       = 'exe'
  url64bit       = $url64
  silentArgs     = '/S'
  validExitCodes = @(0)
  softwareName   = 'Stay Awake*'
  checksumType64 = 'sha256'
  checksum64     = 'PLACEHOLDER'
}

Install-ChocolateyPackage @packageArgs
