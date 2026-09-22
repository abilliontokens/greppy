"""Choose the release matrix. Windows is absent unless every signing secret is set.

include is applied after exclude, so a static Windows include row cannot be
dropped with exclude. This script emits the include list the workflow expands.
"""

from __future__ import annotations

import json
import os
from pathlib import Path

SECRET_ENV = (
    "WINDOWS_SIGNED_WINFSP_DRIVER_BASE64",
    "WINDOWS_SIGNED_WINFSP_CATALOG_BASE64",
    "WINDOWS_SIGNED_WINFSP_DRIVER_CONTRACT_BASE64",
    "WINDOWS_CERTIFICATE_PFX_BASE64",
    "WINDOWS_CERTIFICATE_PASSWORD",
)
WINDOWS_NAME = "windows-x86_64"


def signing_enabled(environ: dict[str, str] | None = None) -> bool:
    values = os.environ if environ is None else environ
    return all(values.get(name) for name in SECRET_ENV)


def filtered_includes(enabled: bool, matrix: dict | None = None) -> dict[str, list]:
    if matrix is None:
        path = Path(__file__).with_name("release_matrix.json")
        matrix = json.loads(path.read_text(encoding="utf-8"))
    chosen = {}
    for key in ("build", "verify"):
        rows = matrix[key]
        if not enabled:
            rows = [row for row in rows if row["name"] != WINDOWS_NAME]
        chosen[key] = rows
    return chosen


def write_github_output(path: Path, environ: dict[str, str] | None = None) -> None:
    enabled = signing_enabled(environ)
    chosen = filtered_includes(enabled)
    with path.open("a", encoding="utf-8") as handle:
        handle.write(f"enabled={'true' if enabled else 'false'}\n")
        handle.write(
            "build_include=" + json.dumps(chosen["build"], separators=(",", ":")) + "\n"
        )
        handle.write(
            "verify_include="
            + json.dumps(chosen["verify"], separators=(",", ":"))
            + "\n"
        )


def main() -> None:
    output = os.environ.get("GITHUB_OUTPUT")
    if not output:
        raise SystemExit("GITHUB_OUTPUT is not set")
    write_github_output(Path(output))


if __name__ == "__main__":
    main()
