//! PDF functions (ISO 32000-1 §7.10): numeric maps from *m* inputs to *n* outputs.
//!
//! Functions are the parameterisation behind several PDF features — tint transforms for
//! `Separation`/`DeviceN` colour (§8.6.6), shading colour gradients (§8.7.4), soft-mask transfer
//! functions, and halftone/transfer curves. Four flavours (§7.10.2–.5):
//!
//! - **Type 2** — exponential interpolation between two output vectors `C0`, `C1` (§7.10.3).
//! - **Type 3** — stitching: a 1-input domain partitioned among *k* subfunctions (§7.10.4).
//! - **Type 0** — sampled: an *m*-dimensional grid of samples, multilinearly interpolated (§7.10.2).
//! - **Type 4** — a PostScript-calculator program over a restricted operator set (§7.10.5).
//!
//! Input is untrusted (DESIGN.md §3.4): parsing is fully fallible (returns `None` on anything
//! malformed), evaluation never panics, inputs/outputs are clamped to `/Domain`/`/Range`, and the
//! type-0 corner count and type-4 step count are bounded.

use pdf_cos::{Dictionary, Name, Object, Stream};
use pdf_filters::decode_stream;

/// Maximum number of inputs for a sampled function: evaluation visits `2^m` grid corners, so this
/// caps that at 256 (anti-DoS). Real functions are almost always 1–2 inputs.
const MAX_SAMPLED_INPUTS: usize = 8;
/// Maximum PostScript-calculator operations executed per evaluation (anti-DoS).
const MAX_PS_STEPS: usize = 100_000;
/// Maximum nesting of type-3 (stitching) subfunctions (§7.10.4), which parse — and later evaluate —
/// recursively. A `/Functions` entry is an indirect reference, so a hostile file can point one back
/// at its own parent; without this bound that recursion exhausts the stack, which aborts the process
/// rather than unwinding (DESIGN.md §3.4). Real files nest one or two levels; the sibling limit for
/// form XObjects is `MAX_FORM_DEPTH` in the `prismpdf` facade.
const MAX_FUNCTION_DEPTH: usize = 8;

/// A parsed, evaluatable PDF function (§7.10).
#[derive(Clone, Debug)]
pub struct Function {
    /// `/Domain`: one (min, max) pair per input; inputs are clamped to it (§7.10.1).
    domain: Vec<(f64, f64)>,
    /// `/Range`: one (min, max) pair per output; outputs are clamped to it. Required for types 0/4,
    /// optional for 2/3.
    range: Option<Vec<(f64, f64)>>,
    kind: Kind,
}

#[derive(Clone, Debug)]
enum Kind {
    /// Type 2 (§7.10.3).
    Exponential { c0: Vec<f64>, c1: Vec<f64>, n: f64 },
    /// Type 3 (§7.10.4).
    Stitching {
        functions: Vec<Function>,
        bounds: Vec<f64>,
        encode: Vec<(f64, f64)>,
    },
    /// Type 0 (§7.10.2).
    Sampled {
        size: Vec<usize>,
        bits_per_sample: u32,
        encode: Vec<(f64, f64)>,
        decode: Vec<(f64, f64)>,
        samples: Vec<u8>,
        outputs: usize,
    },
    /// Type 4 (§7.10.5).
    PostScript { program: Vec<PsTok>, outputs: usize },
}

impl Function {
    /// Number of inputs this function expects (`/Domain` length).
    #[must_use]
    pub fn inputs(&self) -> usize {
        self.domain.len()
    }

    /// Evaluate the function at `input`, returning its outputs. Inputs short of [`Self::inputs`] are
    /// padded with their domain minimum; everything is clamped to `/Domain` and `/Range`.
    #[must_use]
    pub fn eval(&self, input: &[f64]) -> Vec<f64> {
        let x: Vec<f64> = self
            .domain
            .iter()
            .enumerate()
            .map(|(i, &(lo, hi))| clamp(input.get(i).copied().unwrap_or(lo), lo, hi))
            .collect();

        let mut out = match &self.kind {
            Kind::Exponential { c0, c1, n } => eval_exponential(&x, c0, c1, *n),
            Kind::Stitching {
                functions,
                bounds,
                encode,
            } => self.eval_stitching(&x, functions, bounds, encode),
            Kind::Sampled { .. } => self.eval_sampled(&x),
            Kind::PostScript { program, outputs } => eval_postscript(&x, program, *outputs),
        };

        if let Some(range) = &self.range {
            for (o, &(lo, hi)) in out.iter_mut().zip(range) {
                *o = clamp(*o, lo, hi);
            }
        }
        out
    }

    fn eval_stitching(
        &self,
        x: &[f64],
        functions: &[Function],
        bounds: &[f64],
        encode: &[(f64, f64)],
    ) -> Vec<f64> {
        let (d_lo, d_hi) = self.domain[0];
        let v = x[0];
        // Select the subinterval: [d_lo, bounds[0]), [bounds[0], bounds[1]), …, [bounds[k-2], d_hi].
        let mut i = 0;
        while i < bounds.len() && v >= bounds[i] {
            i += 1;
        }
        let lo = if i == 0 { d_lo } else { bounds[i - 1] };
        let hi = if i == bounds.len() { d_hi } else { bounds[i] };
        let (e_lo, e_hi) = encode[i];
        let encoded = interpolate(v, lo, hi, e_lo, e_hi);
        functions[i].eval(&[encoded])
    }

    fn eval_sampled(&self, x: &[f64]) -> Vec<f64> {
        let Kind::Sampled {
            size,
            bits_per_sample,
            encode,
            decode,
            samples,
            outputs,
        } = &self.kind
        else {
            return Vec::new();
        };
        let m = size.len();
        // Map each input through /Encode into grid coordinates, clamped to [0, size-1].
        let mut base = vec![0usize; m];
        let mut frac = vec![0.0f64; m];
        for i in 0..m {
            let (d_lo, d_hi) = self.domain[i];
            let (e_lo, e_hi) = encode[i];
            let e = clamp(
                interpolate(x[i], d_lo, d_hi, e_lo, e_hi),
                0.0,
                (size[i] - 1) as f64,
            );
            let f = e.floor();
            base[i] = f as usize;
            frac[i] = e - f;
        }

        // Multilinear interpolation over the 2^m surrounding grid corners.
        let mut acc = vec![0.0f64; *outputs];
        let corners = 1usize << m;
        for corner in 0..corners {
            let mut weight = 1.0;
            let mut coord = vec![0usize; m];
            for i in 0..m {
                if (corner >> i) & 1 == 1 {
                    coord[i] = (base[i] + 1).min(size[i] - 1);
                    weight *= frac[i];
                } else {
                    coord[i] = base[i];
                    weight *= 1.0 - frac[i];
                }
            }
            if weight == 0.0 {
                continue;
            }
            let lin = linear_index(&coord, size);
            for (j, a) in acc.iter_mut().enumerate() {
                let bit_off = ((lin * outputs + j) as u64) * u64::from(*bits_per_sample);
                *a += weight * sample_at(samples, bit_off, *bits_per_sample);
            }
        }

        // The accumulated values are normalised to [0, 1]; map them through /Decode.
        for (a, &(lo, hi)) in acc.iter_mut().zip(decode) {
            *a = lo + *a * (hi - lo);
        }
        acc
    }
}

fn eval_exponential(x: &[f64], c0: &[f64], c1: &[f64], n: f64) -> Vec<f64> {
    let xn = x[0].powf(n);
    c0.iter().zip(c1).map(|(&a, &b)| a + xn * (b - a)).collect()
}

/// Parse a function from `obj`, resolving any indirect references (subfunctions, the function
/// stream itself) through `resolve`. Returns `None` if the object is not a valid function.
///
/// Type-3 subfunction nesting is bounded (to 8 levels): a `/Functions` array that
/// references its own parent is rejected rather than recursed into (§7.10.4, DESIGN.md §3.4).
#[must_use]
pub fn parse_function(
    obj: &Object,
    resolve: &dyn Fn(&Object) -> Option<Object>,
) -> Option<Function> {
    parse_function_at(obj, resolve, 0)
}

/// Body of [`parse_function`], carrying the nesting depth of type-3 subfunctions.
fn parse_function_at(
    obj: &Object,
    resolve: &dyn Fn(&Object) -> Option<Object>,
    depth: usize,
) -> Option<Function> {
    if depth > MAX_FUNCTION_DEPTH {
        return None;
    }
    let resolved = resolve(obj)?;
    let (dict, stream) = match &resolved {
        Object::Dictionary(d) => (d.clone(), None),
        Object::Stream(s) => (s.dict().clone(), Some(s.clone())),
        _ => return None,
    };

    let ftype = dict.get_integer(&Name::from("FunctionType"))?;
    let domain = pairs(&num_array(&dict, "Domain")?)?;
    let range = match num_array(&dict, "Range") {
        Some(v) => Some(pairs(&v)?),
        None => None,
    };

    let kind = match ftype {
        2 => parse_exponential(&dict)?,
        3 => parse_stitching(&dict, &domain, resolve, depth)?,
        0 => parse_sampled(&dict, stream?, &domain, range.as_ref()?)?,
        4 => parse_postscript(&stream?, range.as_ref()?.len())?,
        _ => return None,
    };

    Some(Function {
        domain,
        range,
        kind,
    })
}

fn parse_exponential(dict: &Dictionary) -> Option<Kind> {
    let n = dict.get(&Name::from("N")).and_then(as_f64)?;
    let c0 = num_array(dict, "C0").unwrap_or_else(|| vec![0.0]);
    let c1 = num_array(dict, "C1").unwrap_or_else(|| vec![1.0]);
    if c0.len() != c1.len() || c0.is_empty() {
        return None;
    }
    Some(Kind::Exponential { c0, c1, n })
}

fn parse_stitching(
    dict: &Dictionary,
    domain: &[(f64, f64)],
    resolve: &dyn Fn(&Object) -> Option<Object>,
    depth: usize,
) -> Option<Kind> {
    if domain.len() != 1 {
        return None; // stitching is single-input (§7.10.4)
    }
    let functions: Vec<Function> = dict
        .get_array(&Name::from("Functions"))?
        .iter()
        .map(|o| parse_function_at(o, resolve, depth + 1))
        .collect::<Option<_>>()?;
    let bounds = num_array(dict, "Bounds").unwrap_or_default();
    let encode = pairs(&num_array(dict, "Encode")?)?;
    if functions.is_empty()
        || encode.len() != functions.len()
        || bounds.len() + 1 != functions.len()
    {
        return None;
    }
    Some(Kind::Stitching {
        functions,
        bounds,
        encode,
    })
}

fn parse_sampled(
    dict: &Dictionary,
    stream: Stream,
    domain: &[(f64, f64)],
    range: &[(f64, f64)],
) -> Option<Kind> {
    let size: Vec<usize> = num_array(dict, "Size")?
        .iter()
        .map(|&s| (s >= 1.0).then_some(s as usize))
        .collect::<Option<_>>()?;
    let m = size.len();
    if m != domain.len() || m == 0 || m > MAX_SAMPLED_INPUTS {
        return None;
    }
    let bits_per_sample = dict.get_integer(&Name::from("BitsPerSample"))? as u32;
    if !matches!(bits_per_sample, 1 | 2 | 4 | 8 | 12 | 16 | 24 | 32) {
        return None;
    }
    let outputs = range.len();
    if outputs == 0 {
        return None;
    }
    // /Encode defaults to [0 (Size_i − 1)] per input; /Decode defaults to /Range.
    let encode = match num_array(dict, "Encode") {
        Some(v) => pairs(&v)?,
        None => size.iter().map(|&s| (0.0, (s - 1) as f64)).collect(),
    };
    let decode = match num_array(dict, "Decode") {
        Some(v) => pairs(&v)?,
        None => range.to_vec(),
    };
    if encode.len() != m || decode.len() != outputs {
        return None;
    }
    let samples = decode_stream(&stream).ok()?;
    Some(Kind::Sampled {
        size,
        bits_per_sample,
        encode,
        decode,
        samples,
        outputs,
    })
}

fn parse_postscript(stream: &Stream, outputs: usize) -> Option<Kind> {
    if outputs == 0 {
        return None;
    }
    let bytes = decode_stream(stream).ok()?;
    let program = parse_ps(&bytes)?;
    Some(Kind::PostScript { program, outputs })
}

// --- shared numeric helpers ---------------------------------------------------------------------

fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    if lo <= hi {
        v.max(lo).min(hi)
    } else {
        v.max(hi).min(lo) // tolerate reversed bounds
    }
}

/// Linear map of `v` from `[lo, hi]` onto `[a, b]` (§7.10.2 "Interpolate").
fn interpolate(v: f64, lo: f64, hi: f64, a: f64, b: f64) -> f64 {
    if hi == lo {
        a
    } else {
        a + (v - lo) * (b - a) / (hi - lo)
    }
}

fn as_f64(o: &Object) -> Option<f64> {
    match o {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(*r),
        _ => None,
    }
}

fn num_array(dict: &Dictionary, key: &str) -> Option<Vec<f64>> {
    dict.get_array(&Name::from(key))?
        .iter()
        .map(as_f64)
        .collect()
}

/// Group a flat `[lo0 hi0 lo1 hi1 …]` array into pairs; `None` if its length is odd.
fn pairs(v: &[f64]) -> Option<Vec<(f64, f64)>> {
    if !v.len().is_multiple_of(2) {
        return None;
    }
    Some(v.chunks_exact(2).map(|c| (c[0], c[1])).collect())
}

fn linear_index(coord: &[usize], size: &[usize]) -> usize {
    // First dimension varies fastest (§7.10.2).
    let mut idx = 0;
    let mut stride = 1;
    for i in 0..size.len() {
        idx += coord[i] * stride;
        stride *= size[i];
    }
    idx
}

/// Read `bits` (≤ 32) big-endian bits at `bit_off` and normalise to `[0, 1]`. Out-of-range bits
/// read as 0, so a truncated sample table degrades gracefully instead of panicking.
fn sample_at(samples: &[u8], bit_off: u64, bits: u32) -> f64 {
    let mut val: u64 = 0;
    for k in 0..u64::from(bits) {
        let pos = bit_off + k;
        let byte = (pos / 8) as usize;
        let bit = if byte < samples.len() {
            (samples[byte] >> (7 - (pos % 8))) & 1
        } else {
            0
        };
        val = (val << 1) | u64::from(bit);
    }
    let max = if bits >= 32 {
        f64::from(u32::MAX)
    } else {
        ((1u64 << bits) - 1) as f64
    };
    val as f64 / max
}

// --- Type 4: PostScript calculator (§7.10.5) ----------------------------------------------------

/// A token in a parsed type-4 program: a number, an operator, or a `{ … }` procedure.
#[derive(Clone, Debug)]
enum PsTok {
    Num(f64),
    Op(PsOp),
    Proc(Vec<PsTok>),
}

#[derive(Clone, Copy, Debug)]
enum PsOp {
    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,
    Idiv,
    Mod,
    Neg,
    Abs,
    Sqrt,
    Sin,
    Cos,
    Atan,
    Exp,
    Ln,
    Log,
    Cvi,
    Cvr,
    Floor,
    Ceiling,
    Round,
    Truncate,
    // Relational / boolean / bitwise
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    And,
    Or,
    Xor,
    Not,
    Bitshift,
    True,
    False,
    // Stack
    Pop,
    Exch,
    Dup,
    Copy,
    Index,
    Roll,
    // Flow
    If,
    IfElse,
}

fn op_from(tok: &[u8]) -> Option<PsOp> {
    Some(match tok {
        b"add" => PsOp::Add,
        b"sub" => PsOp::Sub,
        b"mul" => PsOp::Mul,
        b"div" => PsOp::Div,
        b"idiv" => PsOp::Idiv,
        b"mod" => PsOp::Mod,
        b"neg" => PsOp::Neg,
        b"abs" => PsOp::Abs,
        b"sqrt" => PsOp::Sqrt,
        b"sin" => PsOp::Sin,
        b"cos" => PsOp::Cos,
        b"atan" => PsOp::Atan,
        b"exp" => PsOp::Exp,
        b"ln" => PsOp::Ln,
        b"log" => PsOp::Log,
        b"cvi" => PsOp::Cvi,
        b"cvr" => PsOp::Cvr,
        b"floor" => PsOp::Floor,
        b"ceiling" => PsOp::Ceiling,
        b"round" => PsOp::Round,
        b"truncate" => PsOp::Truncate,
        b"eq" => PsOp::Eq,
        b"ne" => PsOp::Ne,
        b"gt" => PsOp::Gt,
        b"ge" => PsOp::Ge,
        b"lt" => PsOp::Lt,
        b"le" => PsOp::Le,
        b"and" => PsOp::And,
        b"or" => PsOp::Or,
        b"xor" => PsOp::Xor,
        b"not" => PsOp::Not,
        b"bitshift" => PsOp::Bitshift,
        b"true" => PsOp::True,
        b"false" => PsOp::False,
        b"pop" => PsOp::Pop,
        b"exch" => PsOp::Exch,
        b"dup" => PsOp::Dup,
        b"copy" => PsOp::Copy,
        b"index" => PsOp::Index,
        b"roll" => PsOp::Roll,
        b"if" => PsOp::If,
        b"ifelse" => PsOp::IfElse,
        _ => return None,
    })
}

/// Parse a type-4 program: skip to the outer `{`, then parse its body.
fn parse_ps(bytes: &[u8]) -> Option<Vec<PsTok>> {
    let mut i = 0;
    while i < bytes.len() && bytes[i] != b'{' {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    i += 1; // consume the outer '{'
    let (body, _) = parse_ps_block(bytes, i)?;
    Some(body)
}

/// Parse tokens until the matching `}`. Returns the block and the index just past it.
fn parse_ps_block(bytes: &[u8], mut i: usize) -> Option<(Vec<PsTok>, usize)> {
    let mut out = Vec::new();
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'%' {
            while i < bytes.len() && bytes[i] != b'\n' && bytes[i] != b'\r' {
                i += 1;
            }
        } else if c == b'{' {
            let (block, next) = parse_ps_block(bytes, i + 1)?;
            out.push(PsTok::Proc(block));
            i = next;
        } else if c == b'}' {
            return Some((out, i + 1));
        } else {
            let start = i;
            while i < bytes.len()
                && !bytes[i].is_ascii_whitespace()
                && !matches!(bytes[i], b'{' | b'}' | b'%')
            {
                i += 1;
            }
            let tok = &bytes[start..i];
            if let Some(num) = std::str::from_utf8(tok)
                .ok()
                .and_then(|s| s.parse::<f64>().ok())
            {
                out.push(PsTok::Num(num));
            } else {
                out.push(PsTok::Op(op_from(tok)?));
            }
        }
    }
    None // unbalanced braces
}

fn eval_postscript(input: &[f64], program: &[PsTok], outputs: usize) -> Vec<f64> {
    let mut stack: Vec<f64> = input.to_vec();
    let mut procs: Vec<&[PsTok]> = Vec::new();
    let mut steps = 0usize;
    if run_ps(program, &mut stack, &mut procs, &mut steps).is_none() {
        return vec![0.0; outputs];
    }
    // The top `outputs` stack values are the results (last output on top); short stacks pad with 0.
    let mut result = vec![0.0; outputs];
    let have = stack.len().min(outputs);
    for k in 0..have {
        result[outputs - 1 - k] = stack[stack.len() - 1 - k];
    }
    result
}

fn run_ps<'a>(
    toks: &'a [PsTok],
    stack: &mut Vec<f64>,
    procs: &mut Vec<&'a [PsTok]>,
    steps: &mut usize,
) -> Option<()> {
    for t in toks {
        *steps += 1;
        if *steps > MAX_PS_STEPS {
            return None;
        }
        match t {
            PsTok::Num(x) => stack.push(*x),
            PsTok::Proc(p) => procs.push(p),
            PsTok::Op(op) => apply_ps(*op, stack, procs, steps)?,
        }
    }
    Some(())
}

fn apply_ps(
    op: PsOp,
    stack: &mut Vec<f64>,
    procs: &mut Vec<&[PsTok]>,
    steps: &mut usize,
) -> Option<()> {
    let pop = |s: &mut Vec<f64>| s.pop();
    let to_i = |v: f64| v as i64;
    match op {
        PsOp::Add => bin(stack, |a, b| a + b)?,
        PsOp::Sub => bin(stack, |a, b| a - b)?,
        PsOp::Mul => bin(stack, |a, b| a * b)?,
        PsOp::Div => bin(stack, |a, b| if b == 0.0 { 0.0 } else { a / b })?,
        PsOp::Idiv => bin(stack, |a, b| {
            if b == 0.0 {
                0.0
            } else {
                (to_i(a) / to_i(b)) as f64
            }
        })?,
        PsOp::Mod => bin(stack, |a, b| {
            if b == 0.0 {
                0.0
            } else {
                (to_i(a) % to_i(b)) as f64
            }
        })?,
        PsOp::Neg => un(stack, |a| -a)?,
        PsOp::Abs => un(stack, f64::abs)?,
        PsOp::Sqrt => un(stack, |a| a.max(0.0).sqrt())?,
        PsOp::Sin => un(stack, |a| a.to_radians().sin())?, // operand is in degrees (§7.10.5)
        PsOp::Cos => un(stack, |a| a.to_radians().cos())?,
        PsOp::Atan => {
            let den = pop(stack)?;
            let num = pop(stack)?;
            let mut deg = num.atan2(den).to_degrees();
            if deg < 0.0 {
                deg += 360.0;
            }
            stack.push(deg);
        }
        PsOp::Exp => bin(stack, |a, b| a.powf(b))?,
        PsOp::Ln => un(stack, |a| a.max(f64::MIN_POSITIVE).ln())?,
        PsOp::Log => un(stack, |a| a.max(f64::MIN_POSITIVE).log10())?,
        PsOp::Cvi | PsOp::Truncate => un(stack, f64::trunc)?,
        PsOp::Cvr => {}
        PsOp::Floor => un(stack, f64::floor)?,
        PsOp::Ceiling => un(stack, f64::ceil)?,
        PsOp::Round => un(stack, f64::round)?,
        PsOp::Eq => rel(stack, |a, b| a == b)?,
        PsOp::Ne => rel(stack, |a, b| a != b)?,
        PsOp::Gt => rel(stack, |a, b| a > b)?,
        PsOp::Ge => rel(stack, |a, b| a >= b)?,
        PsOp::Lt => rel(stack, |a, b| a < b)?,
        PsOp::Le => rel(stack, |a, b| a <= b)?,
        PsOp::And => bin(stack, |a, b| (to_i(a) & to_i(b)) as f64)?,
        PsOp::Or => bin(stack, |a, b| (to_i(a) | to_i(b)) as f64)?,
        PsOp::Xor => bin(stack, |a, b| (to_i(a) ^ to_i(b)) as f64)?,
        PsOp::Not => un(stack, |a| if a == 0.0 { 1.0 } else { 0.0 })?,
        PsOp::Bitshift => bin(stack, |a, b| {
            let (v, s) = (to_i(a), to_i(b));
            let r = if s >= 0 {
                v << (s & 63)
            } else {
                v >> ((-s) & 63)
            };
            r as f64
        })?,
        PsOp::True => stack.push(1.0),
        PsOp::False => stack.push(0.0),
        PsOp::Pop => {
            pop(stack)?;
        }
        PsOp::Exch => {
            let b = pop(stack)?;
            let a = pop(stack)?;
            stack.push(b);
            stack.push(a);
        }
        PsOp::Dup => {
            let a = *stack.last()?;
            stack.push(a);
        }
        PsOp::Copy => {
            let n = pop(stack)? as i64;
            if n < 0 || n as usize > stack.len() {
                return None;
            }
            let start = stack.len() - n as usize;
            for k in 0..n as usize {
                stack.push(stack[start + k]);
            }
        }
        PsOp::Index => {
            let n = pop(stack)? as i64;
            if n < 0 || n as usize >= stack.len() {
                return None;
            }
            stack.push(stack[stack.len() - 1 - n as usize]);
        }
        PsOp::Roll => {
            let j = pop(stack)? as i64;
            let n = pop(stack)? as i64;
            if n < 0 || n as usize > stack.len() {
                return None;
            }
            let n = n as usize;
            if n > 0 {
                let start = stack.len() - n;
                let shift = j.rem_euclid(n as i64) as usize;
                stack[start..].rotate_right(shift);
            }
        }
        PsOp::If => {
            let proc = procs.pop()?;
            let cond = pop(stack)?;
            if cond != 0.0 {
                run_ps(proc, stack, procs, steps)?;
            }
        }
        PsOp::IfElse => {
            let proc2 = procs.pop()?;
            let proc1 = procs.pop()?;
            let cond = pop(stack)?;
            if cond != 0.0 {
                run_ps(proc1, stack, procs, steps)?;
            } else {
                run_ps(proc2, stack, procs, steps)?;
            }
        }
    }
    Some(())
}

fn un(stack: &mut Vec<f64>, f: impl Fn(f64) -> f64) -> Option<()> {
    let a = stack.pop()?;
    stack.push(f(a));
    Some(())
}

fn bin(stack: &mut Vec<f64>, f: impl Fn(f64, f64) -> f64) -> Option<()> {
    let b = stack.pop()?;
    let a = stack.pop()?;
    stack.push(f(a, b));
    Some(())
}

fn rel(stack: &mut Vec<f64>, f: impl Fn(f64, f64) -> bool) -> Option<()> {
    let b = stack.pop()?;
    let a = stack.pop()?;
    stack.push(if f(a, b) { 1.0 } else { 0.0 });
    Some(())
}

#[cfg(test)]
#[path = "function/tests.rs"]
mod tests;
