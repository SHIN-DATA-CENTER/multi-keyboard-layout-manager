# installer\find-marker.ps1 (design m5b A.10): an exact byte search, the same for the "absent"
# check of release builds and the "present" positive control of development builds.
# Pester 3.4 and 5: Describe / It and plain `throw` only.

Describe 'find-marker.ps1' {

    It 'finds the marker as bytes anywhere in a file, and nothing else' {
        . (Join-Path $PSScriptRoot '..\find-marker.ps1')
        $dir = Join-Path ([IO.Path]::GetTempPath()) ('mklm-marker-' + [guid]::NewGuid().ToString('n'))
        New-Item -ItemType Directory $dir | Out-Null
        try {
            $marker = [Text.Encoding]::ASCII.GetBytes('MKLM-UPDATE-DEV-OVERRIDES!')
            $with = New-Object byte[] 5000
            (New-Object Random 7).NextBytes($with)
            [Array]::Copy($marker, 0, $with, 4321, $marker.Length)
            [IO.File]::WriteAllBytes((Join-Path $dir 'with.exe'), $with)
            $almost = $with.Clone()
            $almost[4321 + 25] = [byte][char]'?'
            [IO.File]::WriteAllBytes((Join-Path $dir 'almost.exe'), $almost)
            $utf16 = [Text.Encoding]::Unicode.GetBytes('MKLM-UPDATE-DEV-OVERRIDES!')
            [IO.File]::WriteAllBytes((Join-Path $dir 'utf16.exe'), $utf16)
            [IO.File]::WriteAllBytes((Join-Path $dir 'edge.exe'), $marker)
            if (-not (Test-DevMarker -File (Join-Path $dir 'with.exe'))) { throw 'with.exe' }
            if (-not (Test-DevMarker -File (Join-Path $dir 'edge.exe'))) { throw 'edge.exe' }
            if (Test-DevMarker -File (Join-Path $dir 'almost.exe')) { throw 'almost.exe' }
            if (Test-DevMarker -File (Join-Path $dir 'utf16.exe')) { throw 'utf16.exe' }

            $script = Join-Path $PSScriptRoot '..\find-marker.ps1'
            & powershell -NoProfile -ExecutionPolicy Bypass -File $script -Expect Present -Path (Join-Path $dir 'with.exe') | Out-Null
            if ($LASTEXITCODE -ne 0) { throw "Present on with.exe: $LASTEXITCODE" }
            & powershell -NoProfile -ExecutionPolicy Bypass -File $script -Expect Absent -Path (Join-Path $dir 'with.exe') | Out-Null
            if ($LASTEXITCODE -ne 1) { throw "Absent on with.exe: $LASTEXITCODE" }
            & powershell -NoProfile -ExecutionPolicy Bypass -File $script -Expect Absent -Path (Join-Path $dir 'almost.exe') | Out-Null
            if ($LASTEXITCODE -ne 0) { throw "Absent on almost.exe: $LASTEXITCODE" }
            & powershell -NoProfile -ExecutionPolicy Bypass -File $script -Expect Absent -Path (Join-Path $dir 'missing.exe') | Out-Null
            if ($LASTEXITCODE -ne 1) { throw "a missing file must fail: $LASTEXITCODE" }
        }
        finally {
            Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
        }
    }
}
