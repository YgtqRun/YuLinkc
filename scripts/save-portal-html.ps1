param(
    [string[]]$Hosts = @("172.26.255.2", "172.26.255.3"),
    [string]$OutDir = (Join-Path $PSScriptRoot "..\docs\portal-html")
)

$ErrorActionPreference = "Continue"
$resolvedOut = [System.IO.Path]::GetFullPath($OutDir)
[System.IO.Directory]::CreateDirectory($resolvedOut) | Out-Null

Write-Host "认证页 HTML 将保存到: $resolvedOut"

foreach ($ip in $Hosts) {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $htmlPath = Join-Path $resolvedOut "$ip-$stamp.html"
    $metaPath = Join-Path $resolvedOut "$ip-$stamp.txt"

    try {
        $resp = Invoke-WebRequest -Uri "http://$ip/" -TimeoutSec 10 -UseBasicParsing
        [System.IO.File]::WriteAllText($htmlPath, $resp.Content, [System.Text.Encoding]::UTF8)
        $meta = @(
            "url: $($resp.BaseResponse.ResponseUri)"
            "status: $($resp.StatusCode)"
            "contentType: $($resp.Headers['Content-Type'])"
            "bytes: $($resp.RawContentLength)"
            "savedAt: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')"
        ) -join "`r`n"
        [System.IO.File]::WriteAllText($metaPath, $meta, [System.Text.Encoding]::UTF8)
        Write-Host "[OK] $ip -> $htmlPath"
    }
    catch {
        Write-Host "[FAIL] $ip : $($_.Exception.Message)"
    }
}

Write-Host "完成。请把结果页面的关键 DOM 差异记录到 docs\联调记录.md"
