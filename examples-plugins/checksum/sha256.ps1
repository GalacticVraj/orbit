# Orbit plugin: SHA-256 of every file passed as an argument, copied to the clipboard.
# One file -> just the hash; several -> "hash  name" lines (sha256sum style).
$ErrorActionPreference = 'Stop'
try {
    $inv = [cultureinfo]::InvariantCulture
    $lines = for ($i = 0; $i -lt $args.Count; $i++) {
        $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $args[$i]).Hash.ToLowerInvariant()
        if ($args.Count -eq 1) { $hash } else { "$hash  $(Split-Path -Leaf $args[$i])" }
        [Console]::Out.WriteLine('progress: ' + (($i + 1) / $args.Count).ToString($inv))
    }
    Set-Clipboard -Value ($lines -join "`r`n")
    if ($args.Count -eq 1) { 'message: SHA-256 copied' } else { "message: $($args.Count) SHA-256 hashes copied" }
} catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
