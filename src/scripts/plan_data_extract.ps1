if ($args.Count -eq 0) {
    Write-Error "Please provide the path to the input file as an argument."
    exit 1
}

. $args[0]

function Get-CACertVersion {
    try {
        $tempFile = New-TemporaryFile
        Invoke-WebRequest -UseBasicParsing -Uri 'https://curl.se/ca/cacert.pem' -OutFile $tempFile
        $datePrefix = "## Certificate data from Mozilla as of: "
        $dateLine = Get-Content $tempFile | Where-Object {
            $_.StartsWith($datePrefix)
        }
        $buildDate = $dateLine.Substring($datePrefix.Length)
        if ($buildDate -match '\b\d{1}\b') {
            $format = 'ddd MMM d HH:mm:ss yyyy GMT'
        } else {
            $format = 'ddd MMM dd HH:mm:ss yyyy GMT'
        }

        $version = [datetime]::ParseExact($buildDate, $format, $null).ToString("yyyy.MM.dd")
        Remove-Item $tempFile -Force | Out-Null

        return $version
    } catch {
        Write-Error "Failed to retrieve the version: $_"
        return $null
    }
}

function Get-CACertSHA256 {
    $tempFile = New-TemporaryFile
    Invoke-WebRequest -UseBasicParsing -Uri 'https://curl.se/ca/cacert.pem.sha256' -OutFile $tempFile
    $shasum = ((Get-Content $tempFile) -split ' ')[0]
    Remove-Item $tempFile -Force | Out-Null

    return $shasum
}

# This is a temporary workaround for cacerts, where we dynamically populate 
# the version as _set_from_downloaded_cacerts_file_ and 
# the SHA256 checksum as _set_from_downloaded_cacerts_sha256_file_.
if ($pkg_name -eq "cacerts") {
    $pkg_version = Get-CACertVersion
    $pkg_shasum = Get-CACertSHA256
}

function ConvertTo-Array($var) {
    if ($null -eq $var) {
        return @() 
    }
    if (-not ($var -is [array])) {
        return @($var)
    }
    return $var
}

# Constructing the JSON manually
$licenses = ConvertTo-Array $pkg_license
$deps = ConvertTo-Array $pkg_deps
$build_deps = ConvertTo-Array $pkg_build_deps

# Convert arrays to JSON format strings
$licensesJson = if ($licenses.Count -gt 0) { "[" + '"{0}"' -f ($licenses -join '", "') + "]"  } else { "[]" }
$depsJson = if ($deps.Count -gt 0) { "[" + '"{0}"' -f ($deps -join '", "') + "]" } else { "[]" }
$buildDepsJson = if ($build_deps.Count -gt 0) { "[" + '"{0}"' -f ($build_deps -join '", "') + "]" } else { "[]" }

# Construct the source block conditionally based on whether $pkg_shasum is empty or not
if (-not [string]::IsNullOrWhiteSpace($pkg_shasum)) {
    $sourceJson = @"

    "source": {
        "url": "$($pkg_source.Trim())",
        "shasum": "$pkg_shasum"
    },
"@
} else {
    $sourceJson = ""  # Empty string if $pkg_shasum is not present
}

if (-not [string]::IsNullOrWhiteSpace($pkg_version)) {
    $_version = $pkg_version.Trim()
} else {
    $_version = "**DYNAMIC**"
}

# Construct the full JSON string
$jsonOutput = @"
{
    "origin": "$pkg_origin",
    "name": "$pkg_name",
    "version": "$_version",$sourceJson
    "licenses": $licensesJson,
    "scaffolding_dep": null,
    "deps": $depsJson,
    "build_deps": $buildDepsJson
}
"@

# Output the JSON to stdout
Write-Output $jsonOutput
