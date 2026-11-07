#!/usr/bin/env bash
set -euo pipefail

# pdl Test Suite Entrypoint
# Usage:
#   ./scripts/test.sh [fast|full|cov] [extra pytest args...]

MODE="${1:-fast}"
shift || true

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${ROOT_DIR}"

VENV_PYTEST="${ROOT_DIR}/.venv/bin/pytest"
if [[ ! -x "${VENV_PYTEST}" ]]; then
    VENV_PYTEST="pytest"
fi

case "${MODE}" in
    fast)
        echo "=== Running Fast Test Suite (Unit + Integration, skipping @pytest.mark.slow) ==="
        "${VENV_PYTEST}" -m "not slow" --durations=15 "$@"
        ;;
    full)
        echo "=== Running Full Test Suite (Unit, Integration, and E2E) ==="
        "${VENV_PYTEST}" -m "" --durations=15 "$@"
        ;;
    cov)
        echo "=== Running Coverage Suite with Core Module Floors (>=80%) ==="
        "${VENV_PYTEST}" \
            -m "" \
            --cov=pdl \
            --cov-report=term-missing \
            --cov-report=xml:coverage.xml \
            --durations=15 \
            "$@"

        echo "=== Verifying Coverage Floors ==="
        "${ROOT_DIR}/.venv/bin/python" -c "
import xml.etree.ElementTree as ET
import sys

tree = ET.parse('coverage.xml')
root = tree.getroot()

core_modules = {
    'auth.py': 80,
    'classifier.py': 80,
    'queue.py': 80,
    'watchdog.py': 80,
    'aria2.py': 72,
}

overall_rate = float(root.attrib.get('line-rate', 0.0)) * 100.0
print(f'Overall test coverage: {overall_rate:.2f}%')
if overall_rate < 62.0:
    print(f'ERROR: Overall coverage {overall_rate:.2f}% is lower than baseline 62.0%', file=sys.stderr)
    sys.exit(1)

failed = False
for cls in root.iter('class'):
    filename = cls.attrib.get('filename', '')
    for mod_name, min_floor in core_modules.items():
        if filename.endswith(mod_name):
            rate = float(cls.attrib.get('line-rate', 0.0)) * 100.0
            print(f'Core module {mod_name}: {rate:.2f}% (floor: {min_floor}%)')
            if rate < min_floor:
                print(f'ERROR: {mod_name} coverage {rate:.2f}% is below required floor {min_floor}%', file=sys.stderr)
                failed = True

if failed:
    sys.exit(1)
print('All coverage floors satisfied!')
"
        ;;
    *)
        echo "Unknown mode: ${MODE}. Expected 'fast', 'full', or 'cov'." >&2
        exit 1
        ;;
esac
