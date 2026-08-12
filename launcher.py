"""Frozen-build entry point.

⚠ PyInstaller runs its entry script as a TOP-LEVEL module, so a script
using relative imports (`from .app import ...`) fails with "attempted
relative import with no known parent package". The entry point therefore
has to import the package absolutely.
"""
from acecm.cli import main

main()
