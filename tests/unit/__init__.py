"""Pure unit test tier for the paradl test suite.

The package marker makes pytest (prepend import mode) import these modules as
``unit.test_*`` so their basenames cannot collide with the still-present legacy
root ``test_*.py`` files during the Wave-3 transition.
"""
