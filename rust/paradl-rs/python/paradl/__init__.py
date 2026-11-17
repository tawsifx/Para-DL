"""Namespace shim so the maturin-built ``_rust_engine`` extension installs under the
existing ``paradl`` package instead of shadowing it.

``extend_path`` keeps the real (editable-installed) ``paradl`` sources on the package
path, and the real package's ``__init__`` is re-executed here so public constants such
as ``VERSION`` remain importable from ``paradl``.
"""

import os as _os
from pkgutil import extend_path as _extend_path

__path__ = _extend_path(__path__, __name__)

for _portion in __path__[1:]:
    _init = _os.path.join(_portion, "__init__.py")
    if _os.path.isfile(_init):
        with open(_init, "rb") as _f:
            exec(compile(_f.read(), _init, "exec"), globals())
        break
