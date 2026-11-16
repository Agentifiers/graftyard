#!/usr/bin/env python3
# roll-up pure half: plants in check_compacted and the fold write; each must fail `--test rollup`. Run from the worktree root.
import subprocess,sys
F='crates/git/src/repo.rs'
M=[('no start check','    if c.from != from {\n','    if false && c.from != from {\n'),
   ('no pack name check','        if p.pack.stored_id() != Some(*id) {','        if false && p.pack.stored_id() != Some(*id) {'),
   ('no end check','    if last_to != c.to {','    if false && last_to != c.to {'),
   ('single allowed','    if c.consumed.len() < 2 {','    if c.consumed.len() < 1 {'),
   ('inline allowed','    if !matches!(c.produced, PackRef::Stored { .. }) {','    if false {'),
   ('compacted not folded','RepoEvent::Compacted(c) => put_rec(w, "compacted", vec![Val::Int(c.from.0 as i64)], c),','RepoEvent::Compacted(_) => {}')]
for n,o,new in M:
    f=F if 'folded' not in n else 'crates/git/src/tiers.rs'
    s=open(f).read(); assert s.count(o)==1,n
    open(f,'w').write(s.replace(o,new))
    r=subprocess.run(['cargo','test','-q','-p','graftyard-git','--test','rollup'],capture_output=True,text=True)
    open(f,'w').write(s)
    print(('CAUGHT' if r.returncode else 'MISSED'),n)
