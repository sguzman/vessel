#!/usr/bin/env fish

# Explicit, persistent WhisperX provisioning for Vessel.
# Normal vessel update never installs Python packages.
# Pass --offline to require everything to already exist in uv's cache.

set -l whisperx_version 3.8.6
set -l uv_args tool install --python 3.12 --no-build "whisperx==$whisperx_version"

if contains --offline $argv
    set uv_args $uv_args --offline
end

uv $uv_args
or exit $status

set -l bin_dir (uv tool dir --bin)
set -l whisperx_bin "$bin_dir/whisperx"

if not test -x "$whisperx_bin"
    echo "WhisperX installed, but executable was not found at $whisperx_bin" >&2
    exit 1
end

"$whisperx_bin" --help >/dev/null
or exit $status

echo "WhisperX $whisperx_version ready at $whisperx_bin"
echo "Python packages were installed from wheels only; source builds were forbidden."
