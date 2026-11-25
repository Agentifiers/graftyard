#!/usr/bin/env python3
"""BF-75's REST artifacts and their zip (`crates/actions/src/artifacts.rs`, `crates/actions/src/zip.rs`;
`decisions/DECISIONS.md`, "`actions-artifacts`", the amendment). The mutants: the expiry counted from the finalize
rather than the create, a retention over 90 days kept, a delete that takes another artifact's chunks, a list that
counts the page rather than the matches, a zip ticket that admits another artifact or an expired one, a gzip file's
deflate stream passing the trailer through, and a plain file's CRC left unchecked against its length. The edge's
routes are checked by `tests/edgeboot/run.sh` (the zips read back, the one-day expiry, the delete), which this file
does not run. Run from the repository root:

    python3 tests/m0carried/plant253.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

A = 'crates/actions/src/artifacts.rs'
Z = 'crates/actions/src/zip.rs'
TA = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--lib', 'artifacts']
TZ = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--lib', 'zip']
MUTANTS = [
    ("the expiry counts from the finalize, not the create", A,
     'let at = created.and_then(|v| v["created"].as_u64()).unwrap_or(now_ms);',
     'let at = now_ms;', TA),
    ("a retention over 90 days is kept", A,
     'Some(days) if days >= 1 => (days as u64).min(RETENTION_DAYS_MAX),',
     'Some(days) if days >= 1 => days as u64,', TA),
    ("an upload pointer's key maps to its job, not its artifact", A,
     '            if job.is_empty() { None } else { named(rest) }\n        }\n        Some(("f", rest))',
     '            if job.is_empty() { None } else { Some(job) }\n        }\n        Some(("f", rest))', TA),
    ("a list counts its page, not its matches", A,
     '    let total = rows.len();\n    let page_rows = rows.into_iter().skip(page.skip()).take(page.per_page).collect();\n'
     '    (total, page_rows, total > page.end())',
     '    let total = rows.len();\n    let page_rows: Vec<ArtifactRow> = rows.into_iter().skip(page.skip()).take(page.per_page).collect();\n'
     '    (page_rows.len(), page_rows, total > page.end())', TA),
    ("a zip ticket admits another artifact", A,
     'ticket["id"].as_i64() == Some(id) && ', '', TA),
    ("a zip ticket admits at its expiry", A,
     'is_some_and(|e| e > now.0 / 1000)', 'is_some_and(|e| e >= now.0 / 1000)', TA),
    ("a plain file's length is not checked at its end", Z,
     '                if pushed != length {', '                if false {', TZ),
    ("a gzip file's ISIZE is not checked against its length", Z,
     '                if isize != length as u32 {', '                if false {', TZ),
]

if __name__ == '__main__':
    run(MUTANTS)
