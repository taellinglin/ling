//! Behavior Trees — the workhorse of game AI, in a flat arena for cache-friendly,
//! allocation-free ticking.
//!
//! A tree is authored as a tiny text DSL and evaluated against a numeric
//! **blackboard** (the agent's perception facts). Ticking returns the name of the
//! action chosen this frame, so the host game stays in control of *how* actions
//! run — the tree only decides *which* one.
//!
//! # DSL
//! ```text
//! selector {
//!     sequence {
//!         ?enemy_visible > 0      # condition: blackboard["enemy_visible"] > 0
//!         ?ammo >= 1
//!         !attack                 # action: choose "attack"
//!     }
//!     sequence {
//!         ?health < 0.3
//!         !flee
//!     }
//!     !patrol                     # fallback action
//! }
//! ```
//! Composites: `selector` (first child that succeeds), `sequence` (all children
//! must succeed), `parallel` (all children ticked, succeeds if all succeed),
//! `not <child>` (invert). Leaves: `?key OP value` conditions
//! (`> < >= <= == !=`) and `!action` actions.

use std::collections::HashMap;

/// Result of ticking a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Failure = 0,
    Success = 1,
    Running = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cmp {
    Gt,
    Lt,
    Ge,
    Le,
    Eq,
    Ne,
}

impl Cmp {
    fn parse(s: &str) -> Option<Cmp> {
        Some(match s {
            ">" => Cmp::Gt,
            "<" => Cmp::Lt,
            ">=" => Cmp::Ge,
            "<=" => Cmp::Le,
            "==" | "=" => Cmp::Eq,
            "!=" | "<>" => Cmp::Ne,
            _ => return None,
        })
    }
    fn test(self, a: f32, b: f32) -> bool {
        match self {
            Cmp::Gt => a > b,
            Cmp::Lt => a < b,
            Cmp::Ge => a >= b,
            Cmp::Le => a <= b,
            Cmp::Eq => (a - b).abs() < f32::EPSILON,
            Cmp::Ne => (a - b).abs() >= f32::EPSILON,
        }
    }
}

enum Node {
    Selector(Vec<usize>),
    Sequence(Vec<usize>),
    Parallel(Vec<usize>),
    Inverter(usize),
    Condition(String, Cmp, f32),
    Action(String),
}

/// A compiled behavior tree plus its blackboard.
pub struct Tree {
    nodes: Vec<Node>,
    root: usize,
    bb: HashMap<String, f32>,
    last_action: String,
    last_status: Status,
}

impl Tree {
    /// Compile a tree from the DSL. Returns `None` on a parse error.
    pub fn parse(src: &str) -> Option<Tree> {
        let toks = lex(src);
        let mut p = Parser { toks: &toks, pos: 0, nodes: Vec::new() };
        let root = p.node()?;
        if p.pos != p.toks.len() {
            return None; // trailing garbage
        }
        Some(Tree {
            nodes: p.nodes,
            root,
            bb: HashMap::new(),
            last_action: String::new(),
            last_status: Status::Failure,
        })
    }

    /// Set a perception fact.
    pub fn set(&mut self, key: &str, value: f32) {
        self.bb.insert(key.to_string(), value);
    }

    /// Read a fact (0.0 if unset).
    pub fn get(&self, key: &str) -> f32 {
        self.bb.get(key).copied().unwrap_or(0.0)
    }

    /// Evaluate the tree once. Returns the chosen action name (`""` if none ran).
    pub fn tick(&mut self) -> String {
        self.last_action.clear();
        self.last_status = self.eval(self.root);
        self.last_action.clone()
    }

    /// Status of the most recent [`Tree::tick`].
    pub fn status(&self) -> Status {
        self.last_status
    }

    fn eval(&mut self, id: usize) -> Status {
        // Borrow the node's structure out by index to avoid holding `&self`
        // across the recursive calls that mutate `self.last_action`.
        match &self.nodes[id] {
            Node::Selector(children) => {
                let kids = children.clone();
                for c in kids {
                    match self.eval(c) {
                        Status::Failure => continue,
                        s => return s,
                    }
                }
                Status::Failure
            },
            Node::Sequence(children) => {
                let kids = children.clone();
                for c in kids {
                    match self.eval(c) {
                        Status::Success => continue,
                        s => return s,
                    }
                }
                Status::Success
            },
            Node::Parallel(children) => {
                let kids = children.clone();
                let mut all = true;
                for c in kids {
                    if self.eval(c) != Status::Success {
                        all = false;
                    }
                }
                if all {
                    Status::Success
                } else {
                    Status::Failure
                }
            },
            Node::Inverter(child) => {
                let c = *child;
                match self.eval(c) {
                    Status::Success => Status::Failure,
                    Status::Failure => Status::Success,
                    Status::Running => Status::Running,
                }
            },
            Node::Condition(key, cmp, val) => {
                let (key, cmp, val) = (key.clone(), *cmp, *val);
                if cmp.test(self.get(&key), val) {
                    Status::Success
                } else {
                    Status::Failure
                }
            },
            Node::Action(name) => {
                self.last_action = name.clone();
                Status::Success
            },
        }
    }
}

// ── Lexer ──────────────────────────────────────────────────────────────────

fn lex(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in src.lines() {
        let line = raw.split('#').next().unwrap_or("");
        for tok in line.split_whitespace() {
            // Split braces that abut other tokens, e.g. `selector{`.
            let mut cur = String::new();
            for ch in tok.chars() {
                if ch == '{' || ch == '}' {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                    out.push(ch.to_string());
                } else {
                    cur.push(ch);
                }
            }
            if !cur.is_empty() {
                out.push(cur);
            }
        }
    }
    out
}

// ── Recursive-descent parser ────────────────────────────────────────────────

struct Parser<'a> {
    toks: &'a [String],
    pos: usize,
    nodes: Vec<Node>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&str> {
        self.toks.get(self.pos).map(|s| s.as_str())
    }
    fn bump(&mut self) -> Option<&str> {
        let t = self.toks.get(self.pos).map(|s| s.as_str());
        if t.is_some() {
            self.pos += 1;
        }
        t
    }
    fn push(&mut self, n: Node) -> usize {
        self.nodes.push(n);
        self.nodes.len() - 1
    }

    fn node(&mut self) -> Option<usize> {
        let head = self.bump()?.to_string();
        match head.as_str() {
            "selector" | "sequence" | "parallel" => {
                if self.bump()? != "{" {
                    return None;
                }
                let mut kids = Vec::new();
                while self.peek()? != "}" {
                    kids.push(self.node()?);
                }
                self.bump(); // consume "}"
                Some(self.push(match head.as_str() {
                    "selector" => Node::Selector(kids),
                    "sequence" => Node::Sequence(kids),
                    _ => Node::Parallel(kids),
                }))
            },
            "not" => {
                let child = self.node()?;
                Some(self.push(Node::Inverter(child)))
            },
            cond if cond.starts_with('?') => {
                let key = cond[1..].to_string();
                let cmp = Cmp::parse(self.bump()?)?;
                let val: f32 = self.bump()?.parse().ok()?;
                Some(self.push(Node::Condition(key, cmp, val)))
            },
            act if act.starts_with('!') => Some(self.push(Node::Action(act[1..].to_string()))),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_picks_first_satisfied_branch() {
        let mut t = Tree::parse(
            "selector {
                sequence { ?enemy > 0 ?ammo >= 1 !attack }
                sequence { ?enemy > 0 !flee }
                !patrol
            }",
        )
        .expect("parse");

        // No enemy → patrol.
        assert_eq!(t.tick(), "patrol");

        // Enemy + ammo → attack.
        t.set("enemy", 1.0);
        t.set("ammo", 3.0);
        assert_eq!(t.tick(), "attack");
        assert_eq!(t.status(), Status::Success);

        // Enemy, no ammo → flee.
        t.set("ammo", 0.0);
        assert_eq!(t.tick(), "flee");
    }

    #[test]
    fn inverter_flips_condition() {
        let mut t =
            Tree::parse("selector { sequence { not ?safe > 0 !hide } !relax }").expect("parse");
        assert_eq!(t.tick(), "hide"); // safe==0 → not(false)=success → hide
        t.set("safe", 1.0);
        assert_eq!(t.tick(), "relax");
    }

    #[test]
    fn comments_and_whitespace_ok() {
        let t = Tree::parse("# a tree\nselector {\n  !idle  # fallback\n}\n");
        assert!(t.is_some());
    }

    #[test]
    fn rejects_malformed() {
        assert!(Tree::parse("selector { !a").is_none()); // unclosed
        assert!(Tree::parse("?x >").is_none()); // missing value
    }
}
