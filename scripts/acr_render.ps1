# Render a RAW through Adobe Camera Raw (Photoshop's plug-in, the same engine Lightroom uses) with the
# settings of the .xmp sidecar beside it, and save a JPEG. The round-trip gate's oracle
# (scripts/roundtrip_gate.py writes the sidecar, forces crs:AlreadyApplied="False" and serialises the
# calls: Photoshop is one COM server, and two renders at once corrupt each other). No dialog opens:
# Photoshop runs the ExtendScript below through COM, with "selected image" settings (= the sidecar). Why ExtendScript and not COM enums:
# the COM enum numbering rejected a value ("应为枚举值"); the script's symbolic constants are the documented API.
# usage: powershell -File acr_render.ps1 <raw> <out.jpg> [size: MAXIMUM|LARGE|MEDIUM|SMALL] [quality 1..12]
param(
    [Parameter(Mandatory = $true)][string]$Raw,
    [Parameter(Mandatory = $true)][string]$Out,
    [string]$Size = "MAXIMUM",
    [int]$Quality = 12
)
$ErrorActionPreference = "Stop"
$rawJs = $Raw -replace '\\', '/'
$outJs = $Out -replace '\\', '/'
$jsx = @"
app.displayDialogs = DialogModes.NO;
var o = new CameraRAWOpenOptions();
o.settings = CameraRAWSettingsType.SELECTEDIMAGE;
o.colorSpace = ColorSpaceType.SRGB;
o.bitsPerChannel = BitsPerChannelType.EIGHT;
o.size = CameraRAWSize.$Size;
var d = app.open(new File("$rawJs"), o);
var j = new JPEGSaveOptions();
j.quality = $Quality;
j.formatOptions = FormatOptions.STANDARDBASELINE;
j.embedColorProfile = true;
d.saveAs(new File("$outJs"), j, true, Extension.LOWERCASE);
var s = d.width + "x" + d.height;
d.close(SaveOptions.DONOTSAVECHANGES);
s;
"@
$app = New-Object -ComObject Photoshop.Application
$r = $app.DoJavaScript($jsx)
Write-Output ("rendered {0} -> {1} ({2} px)" -f $Raw, $Out, $r)
