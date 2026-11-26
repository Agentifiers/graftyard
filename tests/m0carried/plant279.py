#!/usr/bin/env python3
"""AM3-WP4's session logs (`crates/git/src/session_log.rs`, `crates/git/src/taskbox.rs`, `crates/api/src/tasks.rs`;
`decisions/DECISIONS.md`, "`session-log`"). The first mutant is main's read rule: `/log` and the session page answer a
`metadata: read`-only reader 404, not L0-15's 403. Then: the final PATCH not held for the log's push, a push with no
answer given up at once, a refusal retried for ever, the harness's `pushes` kept over the host's, a secret shape or a
minted token not redacted, a secret matched inside a word, the per-step cap not marking `truncated`, the whole-log cap
skipped, another task's stored log served, a session page that does not escape, `fields=prompt` served without
`contents: read`, and a 4xx push answer retried. Run from the repository root:

    python3 tests/m0carried/plant279.py          # every mutant
    python3 tests/m0carried/plant279.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

SL = 'crates/git/src/session_log.rs'
TB = 'crates/git/src/taskbox.rs'
TA = 'crates/api/src/tasks.rs'
TG = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'session_log', '--test', 'taskbox']
TI = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'task_api']
MUTANTS = [
    ("a metadata-only reader gets 404 for /log and the session page, as on main", TA,
     '        return Err(Reply::error(403, "Resource not accessible: session logs need contents: read", DOC));\n',
     '        return Err(Reply::not_found());\n', TI),
    ("the final PATCH is not held while the log's push is owed", TB,
     '        (!(s.is_final() && self.log_due())).then_some(s)\n', '        Some(s)\n', TG),
    ("a push with no answer gives the log up at once", TB,
     '            Pushed::Retry if tries + 1 < LOG_TRIES => LogState::Pending { tries: tries + 1 },\n', '', TG),
    ("a refused push is tried again", TB,
     '            Pushed::Retry if tries + 1 < LOG_TRIES =>', '            Pushed::Retry | Pushed::Refused if tries + 1 < LOG_TRIES =>',
     TG),
    ("a second log report is taken", TB,
     '        if self.log != LogState::Unreported {\n            return false;\n        }\n', '', TG),
    ("acked heads are not recorded for the log's pushes", TB,
     '                    self.pushes.push(h);\n', '                    let _ = h;\n', TG),
    ("a Graftyard ghs_ token with dots is cut at its first dot", SL,
     '    let tok = |b: u8| word(b) || b == b\'.\' || b == b\'-\';\n', '    let tok = |b: u8| word(b) || b == b\'-\';\n', TG),
    ("a token the box minted is not redacted", SL,
     '            s = s.replace(*k, REDACTED);\n', '            let _ = k;\n', TG),
    ("a secret prefix inside a word is redacted", SL,
     '        if (i == 0 || !word(b[i - 1]))\n', '        if true\n', TG),
    ("a PEM private key with no END line is kept", SL,
     '        return Some(end.unwrap_or(s.len()));\n', '        return end;\n', TG),
    ("a step cut to OUTPUT_MAX does not say truncated", SL,
     '        s.truncated |= a || b;\n', '        let _ = (a, b);\n', TG),
    ("the whole log is not cut to LOG_MAX", SL,
     '        if len <= LOG_MAX {\n', '        if len > 0 {\n', TG),
    ("the issue snapshot is not redacted", SL,
     '        redact_value(v, known);\n', '        let _ = v;\n', TG),
    ("another task's stored log is served", TA,
     '    (log.schema == graftyard_git::session_log::SCHEMA && log.id == *id).then_some(log)\n',
     '    (log.schema == graftyard_git::session_log::SCHEMA).then_some(log)\n', TI),
    ("the session page does not escape `<`", TA,
     '            \'<\' => o.push_str("&lt;"),\n', '', TI),
    ("fields=prompt is served without contents: read", TA,
     '    if prompt && !reader.contents {\n', '    if prompt && !reader.metadata {\n', TI),
    ("a 4xx answer to the log push is tried again", SL,
     '        Some(s) if !(200..300).contains(&s) => return Pushed::Refused,\n', '', TG),
]

if __name__ == '__main__':
    run(MUTANTS)
