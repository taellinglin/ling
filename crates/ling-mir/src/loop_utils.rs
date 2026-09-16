use crate::ir::*;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub struct Loop {
    pub header: BasicBlockId,
    pub body: HashSet<BasicBlockId>,
    pub latches: Vec<BasicBlockId>,
    pub exits: Vec<BasicBlockId>,
}

pub fn find_loops(func: &MirFunction) -> Vec<Loop> {
    let mut loops = Vec::new();
    let num_blocks = func.basic_blocks.len();
    if num_blocks == 0 {
        return loops;
    }

    // Predecessors, precomputed once (O(n + e)) instead of the O(n) full-block
    // scan `predecessors()` used to do on every call — this function calls it
    // once per block just below, plus once per block visited by every body-BFS,
    // so the old version was effectively O(n^2) on that alone.
    let preds_map = compute_predecessors(func);
    let idom = compute_idom(func, &preds_map);

    for (n_idx, bb) in func.basic_blocks.iter().enumerate() {
        let n = BasicBlockId(n_idx);
        for &d in &successors(bb) {
            if dominates(&idom, d, n) {
                let mut body = HashSet::default();
                body.insert(d);
                body.insert(n);

                let mut stack = vec![n];
                while let Some(m) = stack.pop() {
                    for &p in &preds_map[m.0] {
                        if p != d && !body.contains(&p) {
                            body.insert(p);
                            stack.push(p);
                        }
                    }
                }

                let mut exits = Vec::new();
                for &b_id in &body {
                    for &s in &successors(&func.basic_blocks[b_id.0]) {
                        if !body.contains(&s) {
                            exits.push(s);
                        }
                    }
                }

                loops.push(Loop { header: d, body, latches: vec![n], exits });
            }
        }
    }
    loops
}

/// Adjacency list of predecessors per block, built in one O(n + e) pass —
/// the shared input both `compute_idom` and `find_loops`'s body-BFS need, so
/// neither has to rescan every block to answer "who points at me".
fn compute_predecessors(func: &MirFunction) -> Vec<Vec<BasicBlockId>> {
    let num_blocks = func.basic_blocks.len();
    let mut preds = vec![Vec::new(); num_blocks];
    for (i, bb) in func.basic_blocks.iter().enumerate() {
        for s in successors(bb) {
            preds[s.0].push(BasicBlockId(i));
        }
    }
    preds
}

/// Reverse postorder over the CFG from the entry block — the traversal order
/// the Cooper/Harvey/Kennedy dominator algorithm below needs to converge in a
/// handful of passes instead of O(n).
fn reverse_postorder(func: &MirFunction) -> Vec<BasicBlockId> {
    let num_blocks = func.basic_blocks.len();
    let mut visited = vec![false; num_blocks];
    let mut postorder = Vec::with_capacity(num_blocks);
    // Explicit stack DFS (avoids recursion depth blowing the stack on a
    // large function): each frame is (block, next successor index to visit).
    let mut stack: Vec<(BasicBlockId, usize)> = vec![(BasicBlockId(0), 0)];
    visited[0] = true;
    while let Some(&mut (b, ref mut next)) = stack.last_mut() {
        let succs = successors(&func.basic_blocks[b.0]);
        if *next < succs.len() {
            let s = succs[*next];
            *next += 1;
            if !visited[s.0] {
                visited[s.0] = true;
                stack.push((s, 0));
            }
        } else {
            postorder.push(b);
            stack.pop();
        }
    }
    postorder.reverse();
    postorder
}

/// Immediate dominators, indexed by block — the standard efficient iterative
/// algorithm (Cooper, Harvey & Kennedy, "A Simple, Fast Dominance
/// Algorithm"), which converges in a small constant number of passes over a
/// reverse-postorder traversal, using O(depth) chain-walking intersections
/// instead of full dominator-set operations. Replaces the previous
/// `compute_dominators`, which stored a full `HashSet<BasicBlockId>` per
/// block and recomputed each one via set intersection every iteration —
/// roughly cubic in block count, ~9-20s per call on a ~1400-block function
/// (this engine's own main-loop closure) and the dominant cost of every JIT
/// compile, since several passes (ConstantPropagation, GVN, LICM, LoopUnroll,
/// LoopVectorizer) call `find_loops` fresh on every optimizer fixpoint
/// iteration. Unreachable blocks (not visited by the RPO walk) keep `None`.
fn compute_idom(func: &MirFunction, preds_map: &[Vec<BasicBlockId>]) -> Vec<Option<BasicBlockId>> {
    let num_blocks = func.basic_blocks.len();
    let rpo = reverse_postorder(func);
    let mut rpo_number = vec![usize::MAX; num_blocks];
    for (i, &b) in rpo.iter().enumerate() {
        rpo_number[b.0] = i;
    }

    let mut idom: Vec<Option<BasicBlockId>> = vec![None; num_blocks];
    idom[0] = Some(BasicBlockId(0));

    let intersect = |mut b1: BasicBlockId,
                      mut b2: BasicBlockId,
                      idom: &[Option<BasicBlockId>]|
     -> BasicBlockId {
        while b1 != b2 {
            while rpo_number[b1.0] > rpo_number[b2.0] {
                b1 = idom[b1.0].expect("finger walks only through already-assigned idoms");
            }
            while rpo_number[b2.0] > rpo_number[b1.0] {
                b2 = idom[b2.0].expect("finger walks only through already-assigned idoms");
            }
        }
        b1
    };

    let mut changed = true;
    while changed {
        changed = false;
        for &b in rpo.iter().skip(1) {
            let mut new_idom: Option<BasicBlockId> = None;
            for &p in &preds_map[b.0] {
                if idom[p.0].is_some() {
                    new_idom = Some(match new_idom {
                        None => p,
                        Some(cur) => intersect(cur, p, &idom),
                    });
                }
            }
            if idom[b.0] != new_idom {
                idom[b.0] = new_idom;
                changed = true;
            }
        }
    }
    idom
}

/// Whether `d` dominates `n` — every path from the entry to `n` passes
/// through `d` — answered by walking `n`'s immediate-dominator chain up to
/// the entry, O(dominator-tree depth) rather than an O(1) lookup into a
/// precomputed O(n)-sized set (the thing that made the old representation
/// expensive to *build*, even though reading it back was cheap).
fn dominates(idom: &[Option<BasicBlockId>], d: BasicBlockId, n: BasicBlockId) -> bool {
    let mut cur = n;
    loop {
        if cur == d {
            return true;
        }
        match idom[cur.0] {
            Some(p) if p != cur => cur = p,
            _ => return false,
        }
    }
}

pub fn clone_blocks(
    func: &mut MirFunction,
    blocks: &HashSet<BasicBlockId>,
) -> HashMap<BasicBlockId, BasicBlockId> {
    let mut map = HashMap::default();

    for &id in blocks {
        let new_id = BasicBlockId(func.basic_blocks.len());
        map.insert(id, new_id);
        func.basic_blocks
            .push(BasicBlock { statements: Vec::new(), terminator: None });
    }

    for &id in blocks {
        let new_id = *map.get(&id).unwrap();
        let old_bb = func.basic_blocks[id.0].clone();

        let mut new_bb = old_bb;
        if let Some(term) = &mut new_bb.terminator {
            match &mut term.kind {
                TerminatorKind::Goto { target } => {
                    if let Some(&new_target) = map.get(target) {
                        *target = new_target;
                    }
                },
                TerminatorKind::SwitchInt { targets, otherwise, .. } => {
                    for (_, t) in targets {
                        if let Some(&new_target) = map.get(t) {
                            *t = new_target;
                        }
                    }
                    if let Some(&new_target) = map.get(otherwise) {
                        *otherwise = new_target;
                    }
                },
                _ => {},
            }
        }
        func.basic_blocks[new_id.0] = new_bb;
    }

    map
}

fn successors(bb: &BasicBlock) -> Vec<BasicBlockId> {
    match &bb.terminator {
        Some(t) => match &t.kind {
            TerminatorKind::Goto { target } => vec![*target],
            TerminatorKind::SwitchInt { targets, otherwise, .. } => {
                let mut s: Vec<_> = targets.iter().map(|(_, b)| *b).collect();
                s.push(*otherwise);
                s
            },
            _ => vec![],
        },
        None => vec![],
    }
}
