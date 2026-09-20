#!/usr/bin/env python3
"""Print the benchmark environment fingerprint for this machine.

The fingerprint is the exact environment a result was measured in:
torch version, CUDA version, driver version, GPU name, GPU arch, and the
python version running this script. Results from different fingerprints
must never be compared; scoring against a stale baseline is refused.

Run this under the kb-env python so the torch/CUDA values are the ones
benchmarks actually run with:

    ~/.local/share/farmerbob/kb-env/bin/python shims/kb_fingerprint.py

Output: one fingerprint string, e.g.
    torch=2.14.0+cu130;cuda=13.0;driver=610.57.04;gpu=RTX5090;arch=sm_120;python=3.14.7
"""

import re
import subprocess
import sys


def _sm_tag(capability):
    major, minor = capability
    return f"sm_{major}{minor}"


def main() -> int:
    try:
        import torch
    except ImportError as e:
        print(f"fingerprint: no torch: {e}", file=sys.stderr)
        return 2
    if not torch.cuda.is_available():
        print("fingerprint: cuda not available", file=sys.stderr)
        return 2
    torch_version = torch.__version__
    cuda_version = str(torch.version.cuda)
    try:
        out = subprocess.run(
            ["nvidia-smi", "--query-gpu=name,driver_version", "--format=csv,noheader"],
            capture_output=True,
            text=True,
            timeout=15,
            check=True,
        )
        name, driver = [p.strip() for p in out.stdout.strip().split(",", 1)]
    except Exception as e:
        print(f"fingerprint: nvidia-smi failed: {e}", file=sys.stderr)
        return 2
    gpu_name = re.sub(r"\s+", "", re.sub(r"^NVIDIA GeForce ", "", name))
    try:
        arch = _sm_tag(torch.cuda.get_device_capability(0))
    except Exception as e:
        print(f"fingerprint: cannot read capability: {e}", file=sys.stderr)
        return 2
    python_version = ".".join(str(p) for p in sys.version_info[:3])
    print(
        f"torch={torch_version};cuda={cuda_version};driver={driver};"
        f"gpu={gpu_name};arch={arch};python={python_version}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
