if ($args.Count -eq 0) {
    Write-Error "Please provide the path to the input file as an argument."
    exit 1
}

. $args[0]

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
    $pkg_version = $pkg_version.Trim()
} else {
    $pkg_version = "**DYNAMIC**"
}

# Construct the full JSON string
$jsonOutput = @"
{
    "origin": "$pkg_origin",
    "name": "$pkg_name",
    "version": "$pkg_version",$sourceJson
    "licenses": $licensesJson,
    "scaffolding_dep": null,
    "deps": $depsJson,
    "build_deps": $buildDepsJson
}
"@

# Output the JSON to stdout
Write-Output $jsonOutput
