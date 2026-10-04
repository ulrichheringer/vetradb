#!/usr/bin/env python3
"""Inventory integrity only; does not simulate PostgreSQL or qualify a driver."""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def check():
    matrix = json.loads((ROOT / 'docs/specs/postgresql-matrix.json').read_text())
    if (matrix['postgresql_major'], matrix['protocol']) != (17, 196608):
        raise ValueError('PostgreSQL reference changed without contract review')
    ids = set()
    for row in matrix['features']:
        if row['id'] in ids or row['status'] not in {'planned', 'tested', 'unsupported', 'deferred'}:
            raise ValueError('duplicate feature or invalid status')
        ids.add(row['id'])
        if not row['issues'] or not row['contract']:
            raise ValueError('missing feature ownership/contract')
        if row['status'] == 'tested':
            required = {'driver', 'runtime', 'reference', 'artifact', 'platform'}
            if not row['evidence'] or not required.issubset(row['versions']) or any(not v for v in row['versions'].values()):
                raise ValueError('tested conformance requires exact versions and evidence')
    cases = json.loads((ROOT / 'docs/fixtures/conformance/protocol-cases.json').read_text())
    case_ids = set()
    for case in cases:
        if case['id'] in case_ids or not case['flow'] or not case['effect'] or case['status'] != 'planned':
            raise ValueError('invalid acceptance case')
        case_ids.add(case['id'])
        if case['ready_state'] not in {None, 'I', 'T', 'E'}:
            raise ValueError('invalid ReadyForQuery state')
        if case['sqlstate'] is not None and (len(case['sqlstate']) != 5 or not case['sqlstate'].isalnum()):
            raise ValueError('invalid SQLSTATE')
    required_cases = {'startup-success', 'auth-failure', 'explicit-error', 'failed-transaction', 'savepoint-recovery', 'implicit-multi-error', 'extended-error-drain', 'copy-invalid', 'unsupported-setting', 'unsupported-extension', 'cancellation'}
    if not required_cases.issubset(case_ids):
        raise ValueError('negative/state conformance coverage lost')
    return len(ids), len(cases)


if __name__ == '__main__':
    rows, cases = check()
    print(f'Contract inventory: {rows} features, {cases} planned protocol cases; no driver qualification claimed.')
