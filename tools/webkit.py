#!/usr/bin/env python3
"""Compatibility notice: the independent WebKit builder belongs to mochiOS/libs."""
import sys
if __name__ == '__main__':
    print('WebKit builds moved to mochiOS/libs: use its tools/webkit.py. Terra consumes the engine package; it does not build it.', file=sys.stderr)
    raise SystemExit(2)
