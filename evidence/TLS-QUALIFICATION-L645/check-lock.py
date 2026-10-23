#!/usr/bin/env python3
"""Read-only qualification check; Python 3.11+ stdlib, not a service dependency."""
import argparse
import json
import re
import tomllib
from pathlib import Path

FLOOR=(0,23,46)

def check(path: Path) -> dict:
    lock=tomllib.loads(path.read_text())
    versions=[p['version'] for p in lock.get('package',[]) if p.get('name')=='rustls']
    refused=[]
    for version in versions:
        match=re.fullmatch(r'(\d+)\.(\d+)\.(\d+)',version)
        if match is None or tuple(map(int,match.groups()))<FLOOR:
            refused.append(version)
    return {'rustls_versions':versions,'minimum':'0.23.46','refused_versions':refused,'pass':bool(versions) and not refused}

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('lockfile',type=Path)
    args=parser.parse_args()
    result=check(args.lockfile)
    print(json.dumps(result,indent=2))
    raise SystemExit(0 if result['pass'] else 1)
if __name__=='__main__':main()
