#!/usr/bin/env python3
"""R23 release M3 (`decisions/OPEN-QUESTIONS.md`, R23; `decisions/DECISIONS.md`, "`r23-m3`"): `graftyard runners init`
deletes the Sandbox SDK classes that ran hosted runners before `RunnerBox` with migration `v3`, then their two container
applications, and refuses before it writes anything while either application has a container that is not stopped
(`cli/src/runners.rs`, `idle_sandbox_apps` and `delete_apps`). The first mutant is main's rule: no `v3`, so an account
at `v2` keeps both classes and their applications. Run from the repository root:

    python3 tests/m0carried/plant273.py          # every mutant
    python3 tests/m0carried/plant273.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

R = 'cli/src/runners.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard', '--bin', 'graftyard', 'runners::']
MUTANTS = [
    ("an account at v2 is left at v2 with its Sandbox classes, as on main", R,
     '            meta["migrations"] = json!({ "old_tag": "v2", "new_tag": MIGRATION_TAG, "steps": [drop_sandboxes] })\n',
     '\n', T),
    ("a running Sandbox container does not refuse the release", R,
     '        if !live.is_empty() {\n', '        if false && !live.is_empty() {\n', T),
    ("a stopping container counts as done", R,
     'const DONE_STATES: [&str; 4] = ["inactive", "stopped", "failed", "deleted"];',
     'const DONE_STATES: [&str; 5] = ["inactive", "stopped", "failed", "deleted", "stopping"];', T),
    ("the refusal comes after the upload", R,
     '    let sandbox_apps = idle_sandbox_apps(cf, &spec)?;\n    cf.put_script(&spec.script, &meta, &bundle.modules())?;\n',
     '    cf.put_script(&spec.script, &meta, &bundle.modules())?;\n    let sandbox_apps = idle_sandbox_apps(cf, &spec)?;\n',
     T),
    ("the applications are deleted before the migration that deletes their classes", R,
     '    cf.put_script(&spec.script, &meta, &bundle.modules())?;\n    delete_apps(cf, &sandbox_apps)?;\n',
     '    delete_apps(cf, &sandbox_apps)?;\n    cf.put_script(&spec.script, &meta, &bundle.modules())?;\n', T),
    ("the Sandbox applications are never deleted", R,
     '        cf.delete_container_app(id)?;\n', '        let _ = id;\n', T),
    ("RunnerBox's application is deleted too", R,
     '        if !SANDBOX_CLASSES.iter().any(|c| name == spec.app(c)) {\n',
     '        if !SANDBOX_CLASSES.iter().any(|c| name == spec.app(c)) && !name.ends_with("runnerbox") {\n', T),
    ("upgrade uploads a version beside the Sandbox classes", R,
     '                Some(t) if t == MIGRATION_TAG => {}\n', '                Some(_) => {}\n', T),
]

if __name__ == '__main__':
    run(MUTANTS)
