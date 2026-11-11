#!/usr/bin/env python3
"""Exercise default configuration discovery without accessing USB hardware."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def list_profiles(driver, environment, status):
    result = subprocess.run(
        [driver, "--list-profiles"], env=environment,
        capture_output=True, text=True, timeout=10,
    )
    assert result.returncode == status, (result.returncode, result.stderr)
    if status == 0:
        assert "custom-2:" in result.stdout
    return result


for use_xdg in (True, False):
    with tempfile.TemporaryDirectory() as directory:
        environment = os.environ.copy()
        environment["HOME"] = directory
        configuration = Path(directory) / ("xdg" if use_xdg else ".config")
        if use_xdg:
            environment["XDG_CONFIG_HOME"] = str(configuration)
        else:
            environment.pop("XDG_CONFIG_HOME", None)

        list_profiles(sys.argv[1], environment, 0)

        obsolete = configuration / "ozzy-a16" / "config.json"
        obsolete.parent.mkdir(parents=True)
        obsolete.write_text("invalid configuration")
        list_profiles(sys.argv[1], environment, 0)

        current = configuration / "open-realiser-a16-driver" / "config.json"
        current.parent.mkdir(parents=True)
        current.write_text("invalid configuration")
        failed = list_profiles(sys.argv[1], environment, 2)
        assert str(current) in failed.stderr

        current.write_text('{"missing_speakers": []}')
        list_profiles(sys.argv[1], environment, 0)
