use super::*;
use pdf_cos::Array;

/// Resolver for direct objects (tests build everything inline, no indirect references).
fn direct(o: &Object) -> Option<Object> {
    Some(o.clone())
}

fn num(v: f64) -> Object {
    Object::Real(v)
}

fn arr(vals: &[f64]) -> Object {
    Object::Array(Array::from(
        vals.iter().map(|&v| num(v)).collect::<Vec<_>>(),
    ))
}

fn approx(a: &[f64], b: &[f64]) {
    assert_eq!(a.len(), b.len(), "len {a:?} vs {b:?}");
    for (x, y) in a.iter().zip(b) {
        assert!((x - y).abs() < 1e-6, "{a:?} vs {b:?}");
    }
}

#[test]
fn type2_exponential_linear_and_squared() {
    // Linear (N=1) from C0=[0,0,0] to C1=[1,0.5,0].
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(2));
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    d.insert(Name::from("N"), Object::Integer(1));
    d.insert(Name::from("C0"), arr(&[0.0, 0.0, 0.0]));
    d.insert(Name::from("C1"), arr(&[1.0, 0.5, 0.0]));
    let f = parse_function(&Object::Dictionary(d), &direct).unwrap();
    approx(&f.eval(&[0.0]), &[0.0, 0.0, 0.0]);
    approx(&f.eval(&[0.5]), &[0.5, 0.25, 0.0]);
    approx(&f.eval(&[1.0]), &[1.0, 0.5, 0.0]);

    // N=2: output = x^2.
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(2));
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    d.insert(Name::from("N"), Object::Integer(2));
    let f = parse_function(&Object::Dictionary(d), &direct).unwrap();
    approx(&f.eval(&[0.5]), &[0.25]); // C0/C1 default to [0]/[1]
}

#[test]
fn type2_clamps_to_domain_and_range() {
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(2));
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    d.insert(Name::from("Range"), arr(&[0.0, 0.5]));
    d.insert(Name::from("N"), Object::Integer(1));
    let f = parse_function(&Object::Dictionary(d), &direct).unwrap();
    approx(&f.eval(&[2.0]), &[0.5]); // input clamped to 1.0, output clamped to 0.5
    approx(&f.eval(&[-1.0]), &[0.0]);
}

#[test]
fn type3_stitching_selects_subfunction() {
    // Two linear pieces over [0,1]: first maps [0,0.5)→[0,1] then x; second [0.5,1]→[0,1] then 1-x.
    let mut sub0 = Dictionary::new();
    sub0.insert(Name::from("FunctionType"), Object::Integer(2));
    sub0.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    sub0.insert(Name::from("N"), Object::Integer(1)); // x

    let mut sub1 = Dictionary::new();
    sub1.insert(Name::from("FunctionType"), Object::Integer(2));
    sub1.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    sub1.insert(Name::from("N"), Object::Integer(1));
    sub1.insert(Name::from("C0"), arr(&[1.0]));
    sub1.insert(Name::from("C1"), arr(&[0.0])); // 1 - x

    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(3));
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    d.insert(
        Name::from("Functions"),
        Object::Array(Array::from(vec![
            Object::Dictionary(sub0),
            Object::Dictionary(sub1),
        ])),
    );
    d.insert(Name::from("Bounds"), arr(&[0.5]));
    d.insert(Name::from("Encode"), arr(&[0.0, 1.0, 0.0, 1.0]));
    let f = parse_function(&Object::Dictionary(d), &direct).unwrap();
    // First half encodes [0,0.5)→[0,1]: at 0.25 → encoded 0.5 → sub0 = 0.5.
    approx(&f.eval(&[0.25]), &[0.5]);
    // Second half encodes [0.5,1]→[0,1]: at 0.75 → encoded 0.5 → sub1 = 1-0.5 = 0.5.
    approx(&f.eval(&[0.75]), &[0.5]);
    // At the far end, encoded 1.0 → sub1 = 0.0.
    approx(&f.eval(&[1.0]), &[0.0]);
}

fn sampled_stream(dict: Dictionary, samples: Vec<u8>) -> Object {
    Object::Stream(Stream::new(dict, samples))
}

#[test]
fn type0_sampled_1d_8bit_interpolates() {
    // 1 input, 1 output, 3 samples [0, 128, 255] over domain [0,1], range [0,1].
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(0));
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    d.insert(Name::from("Range"), arr(&[0.0, 1.0]));
    d.insert(
        Name::from("Size"),
        Object::Array(Array::from(vec![Object::Integer(3)])),
    );
    d.insert(Name::from("BitsPerSample"), Object::Integer(8));
    let f = parse_function(&sampled_stream(d, vec![0, 128, 255]), &direct).unwrap();
    approx(&f.eval(&[0.0]), &[0.0]); // sample 0
    approx(&f.eval(&[1.0]), &[1.0]); // sample 2 = 255/255
    approx(&f.eval(&[0.5]), &[128.0 / 255.0]); // exactly sample 1
    // Halfway between samples 0 and 1 (grid coord 0.5): (0 + 128)/2 /255.
    approx(&f.eval(&[0.25]), &[(64.0) / 255.0]);
}

#[test]
fn type0_decode_remaps_output() {
    // Same table but /Decode maps [0,1] sample range onto [10,20].
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(0));
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    d.insert(Name::from("Range"), arr(&[0.0, 100.0]));
    d.insert(Name::from("Decode"), arr(&[10.0, 20.0]));
    d.insert(
        Name::from("Size"),
        Object::Array(Array::from(vec![Object::Integer(2)])),
    );
    d.insert(Name::from("BitsPerSample"), Object::Integer(8));
    let f = parse_function(&sampled_stream(d, vec![0, 255]), &direct).unwrap();
    approx(&f.eval(&[0.0]), &[10.0]);
    approx(&f.eval(&[1.0]), &[20.0]);
    approx(&f.eval(&[0.5]), &[15.0]);
}

#[test]
fn type4_arithmetic_and_stack() {
    // { 2 mul 1 add }: y = 2x + 1.
    let prog = b"{ 2 mul 1 add }".to_vec();
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(4));
    d.insert(Name::from("Domain"), arr(&[0.0, 10.0]));
    d.insert(Name::from("Range"), arr(&[0.0, 100.0]));
    let f = parse_function(&Object::Stream(Stream::new(d, prog)), &direct).unwrap();
    approx(&f.eval(&[3.0]), &[7.0]);
}

#[test]
fn type4_two_outputs_and_dup() {
    // { dup 2 mul }: inputs [x] → but domain is 1-in; outputs [x, 2x].
    let prog = b"{ dup 2 mul }".to_vec();
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(4));
    d.insert(Name::from("Domain"), arr(&[0.0, 10.0]));
    d.insert(Name::from("Range"), arr(&[0.0, 10.0, 0.0, 20.0]));
    let f = parse_function(&Object::Stream(Stream::new(d, prog)), &direct).unwrap();
    approx(&f.eval(&[3.0]), &[3.0, 6.0]);
}

#[test]
fn type4_ifelse_and_comparison() {
    // { dup 0.5 lt { pop 0 } { pop 1 } ifelse }: a step function (threshold at 0.5).
    let prog = b"{ dup 0.5 lt { pop 0 } { pop 1 } ifelse }".to_vec();
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(4));
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    d.insert(Name::from("Range"), arr(&[0.0, 1.0]));
    let f = parse_function(&Object::Stream(Stream::new(d, prog)), &direct).unwrap();
    approx(&f.eval(&[0.2]), &[0.0]);
    approx(&f.eval(&[0.8]), &[1.0]);
}

#[test]
fn type4_roll_reorders() {
    // { 3 1 roll }: rotate top 3 by 1 → [a b c] becomes [c a b]; top output is b.
    let prog = b"{ 3 1 roll }".to_vec();
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(4));
    d.insert(Name::from("Domain"), arr(&[0.0, 9.0, 0.0, 9.0, 0.0, 9.0]));
    d.insert(Name::from("Range"), arr(&[0.0, 9.0, 0.0, 9.0, 0.0, 9.0]));
    let f = parse_function(&Object::Stream(Stream::new(d, prog)), &direct).unwrap();
    // inputs pushed [1,2,3]; roll → [3,1,2]; outputs read in order = [3,1,2].
    approx(&f.eval(&[1.0, 2.0, 3.0]), &[3.0, 1.0, 2.0]);
}

/// Build and evaluate a type-4 program with `n_in` inputs / `n_out` outputs over a wide
/// domain/range (so nothing is clamped), returning its outputs.
fn ps(prog: &[u8], n_in: usize, n_out: usize, input: &[f64]) -> Vec<f64> {
    let dom: Vec<f64> = (0..n_in).flat_map(|_| [-1e9, 1e9]).collect();
    let rng: Vec<f64> = (0..n_out).flat_map(|_| [-1e9, 1e9]).collect();
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(4));
    d.insert(Name::from("Domain"), arr(&dom));
    d.insert(Name::from("Range"), arr(&rng));
    let f = parse_function(&Object::Stream(Stream::new(d, prog.to_vec())), &direct).unwrap();
    f.eval(input)
}

#[test]
fn inputs_reports_domain_length() {
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(2));
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0, 0.0, 1.0]));
    d.insert(Name::from("N"), Object::Integer(1));
    let f = parse_function(&Object::Dictionary(d), &direct).unwrap();
    assert_eq!(f.inputs(), 2);
}

#[test]
fn type4_unary_arithmetic_operators() {
    // Each consumes the single input and leaves one result (§7.10.5).
    approx(&ps(b"{ neg }", 1, 1, &[5.0]), &[-5.0]);
    approx(&ps(b"{ abs }", 1, 1, &[-3.0]), &[3.0]);
    approx(&ps(b"{ sqrt }", 1, 1, &[9.0]), &[3.0]);
    approx(&ps(b"{ sqrt }", 1, 1, &[-4.0]), &[0.0]); // clamped to >= 0 before sqrt
    approx(&ps(b"{ sin }", 1, 1, &[90.0]), &[1.0]); // degrees
    approx(&ps(b"{ cos }", 1, 1, &[0.0]), &[1.0]);
    approx(&ps(b"{ ln }", 1, 1, &[1.0]), &[0.0]);
    approx(&ps(b"{ log }", 1, 1, &[100.0]), &[2.0]);
    approx(&ps(b"{ floor }", 1, 1, &[3.7]), &[3.0]);
    approx(&ps(b"{ ceiling }", 1, 1, &[3.2]), &[4.0]);
    approx(&ps(b"{ round }", 1, 1, &[3.5]), &[4.0]);
    approx(&ps(b"{ truncate }", 1, 1, &[3.9]), &[3.0]);
    approx(&ps(b"{ cvi }", 1, 1, &[3.9]), &[3.0]);
    approx(&ps(b"{ cvr }", 1, 1, &[3.7]), &[3.7]); // no-op
    approx(&ps(b"{ not }", 1, 1, &[0.0]), &[1.0]);
    approx(&ps(b"{ not }", 1, 1, &[5.0]), &[0.0]);
}

#[test]
fn type4_binary_arithmetic_and_bitwise_operators() {
    approx(&ps(b"{ sub }", 2, 1, &[5.0, 3.0]), &[2.0]);
    approx(&ps(b"{ div }", 2, 1, &[6.0, 2.0]), &[3.0]);
    approx(&ps(b"{ div }", 2, 1, &[1.0, 0.0]), &[0.0]); // divide-by-zero guarded
    approx(&ps(b"{ idiv }", 2, 1, &[7.0, 2.0]), &[3.0]);
    approx(&ps(b"{ idiv }", 2, 1, &[5.0, 0.0]), &[0.0]);
    approx(&ps(b"{ mod }", 2, 1, &[7.0, 3.0]), &[1.0]);
    approx(&ps(b"{ mod }", 2, 1, &[5.0, 0.0]), &[0.0]);
    approx(&ps(b"{ exp }", 2, 1, &[2.0, 3.0]), &[8.0]);
    approx(&ps(b"{ and }", 2, 1, &[6.0, 3.0]), &[2.0]);
    approx(&ps(b"{ or }", 2, 1, &[6.0, 1.0]), &[7.0]);
    approx(&ps(b"{ xor }", 2, 1, &[6.0, 3.0]), &[5.0]);
    approx(&ps(b"{ bitshift }", 2, 1, &[1.0, 3.0]), &[8.0]); // left shift
    approx(&ps(b"{ bitshift }", 2, 1, &[8.0, -2.0]), &[2.0]); // right shift
}

#[test]
fn type4_atan_normalises_to_zero_to_360() {
    approx(&ps(b"{ atan }", 2, 1, &[1.0, 1.0]), &[45.0]);
    approx(&ps(b"{ atan }", 2, 1, &[0.0, 1.0]), &[0.0]);
    // num<0 → atan2 negative → +360 branch.
    approx(&ps(b"{ atan }", 2, 1, &[-1.0, 1.0]), &[315.0]);
}

#[test]
fn type4_relational_and_boolean_operators() {
    approx(&ps(b"{ eq }", 2, 1, &[1.0, 1.0]), &[1.0]);
    approx(&ps(b"{ ne }", 2, 1, &[1.0, 2.0]), &[1.0]);
    approx(&ps(b"{ gt }", 2, 1, &[2.0, 1.0]), &[1.0]);
    approx(&ps(b"{ ge }", 2, 1, &[2.0, 2.0]), &[1.0]);
    approx(&ps(b"{ le }", 2, 1, &[1.0, 2.0]), &[1.0]);
    approx(&ps(b"{ pop true }", 1, 1, &[0.0]), &[1.0]);
    approx(&ps(b"{ pop false }", 1, 1, &[0.0]), &[0.0]);
}

#[test]
fn type4_stack_operators() {
    // exch swaps the top two; with two outputs the order flips.
    approx(&ps(b"{ exch }", 2, 2, &[1.0, 2.0]), &[2.0, 1.0]);
    // copy duplicates the top n elements.
    approx(&ps(b"{ 1 copy }", 1, 2, &[7.0]), &[7.0, 7.0]);
    // index n pushes a copy of the element n below the top.
    approx(&ps(b"{ 10 20 1 index }", 1, 1, &[0.0]), &[10.0]);
    // if executes its proc when the condition is true, skips it when false.
    approx(&ps(b"{ pop 1 { 42 } if }", 1, 1, &[0.0]), &[42.0]);
    approx(&ps(b"{ pop 0 { 42 } if }", 1, 1, &[0.0]), &[0.0]);
}

#[test]
fn type4_stack_operator_errors_evaluate_to_zero() {
    // copy/index/roll with out-of-range counts abort cleanly → zero-filled output, no panic.
    approx(&ps(b"{ -1 copy }", 1, 1, &[7.0]), &[0.0]);
    approx(&ps(b"{ 5 index }", 1, 1, &[7.0]), &[0.0]);
    approx(&ps(b"{ 5 1 roll }", 1, 1, &[7.0]), &[0.0]);
    // A program comment (`%`) is skipped during parsing.
    approx(&ps(b"{ % a comment\n 2 mul }", 1, 1, &[3.0]), &[6.0]);
}

#[test]
fn type0_high_bit_depth_and_truncated_samples() {
    // 32-bit samples exercise the wide-sample normalisation path.
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(0));
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    d.insert(Name::from("Range"), arr(&[0.0, 1.0]));
    d.insert(
        Name::from("Size"),
        Object::Array(Array::from(vec![Object::Integer(2)])),
    );
    d.insert(Name::from("BitsPerSample"), Object::Integer(32));
    // Two 32-bit big-endian samples: 0 and u32::MAX → 0.0 and 1.0.
    let samples = vec![0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF];
    let f = parse_function(&sampled_stream(d, samples), &direct).unwrap();
    approx(&f.eval(&[0.0]), &[0.0]);
    approx(&f.eval(&[1.0]), &[1.0]);

    // A sample table shorter than Size×outputs reads missing bits as 0 instead of panicking.
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(0));
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    d.insert(Name::from("Range"), arr(&[0.0, 1.0]));
    d.insert(
        Name::from("Size"),
        Object::Array(Array::from(vec![Object::Integer(4)])),
    );
    d.insert(Name::from("BitsPerSample"), Object::Integer(8));
    let f = parse_function(&sampled_stream(d, vec![255]), &direct).unwrap(); // only 1 of 4 samples
    approx(&f.eval(&[0.0]), &[1.0]); // sample 0 present
    approx(&f.eval(&[1.0]), &[0.0]); // sample 3 missing → 0
}

#[test]
fn eval_clamps_reversed_domain_and_constant_domain() {
    // Reversed /Domain bounds (lo > hi) are tolerated by clamp's else branch.
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(2));
    d.insert(Name::from("Domain"), arr(&[1.0, 0.0]));
    d.insert(Name::from("N"), Object::Integer(1));
    let f = parse_function(&Object::Dictionary(d), &direct).unwrap();
    approx(&f.eval(&[0.5]), &[0.5]);

    // A degenerate /Domain (lo == hi) makes interpolate return the encode minimum.
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(0));
    d.insert(Name::from("Domain"), arr(&[0.0, 0.0]));
    d.insert(Name::from("Range"), arr(&[0.0, 1.0]));
    d.insert(
        Name::from("Size"),
        Object::Array(Array::from(vec![Object::Integer(2)])),
    );
    d.insert(Name::from("BitsPerSample"), Object::Integer(8));
    let f = parse_function(&sampled_stream(d, vec![0, 255]), &direct).unwrap();
    approx(&f.eval(&[0.0]), &[0.0]);
}

#[test]
fn parse_rejects_each_malformed_shape() {
    // Not a dictionary or stream at all.
    assert!(parse_function(&Object::Integer(5), &direct).is_none());

    // /Domain with an odd element count (cannot be grouped into pairs).
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(2));
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0, 2.0]));
    d.insert(Name::from("N"), Object::Integer(1));
    assert!(parse_function(&Object::Dictionary(d), &direct).is_none());

    // /Domain containing a non-numeric element.
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(2));
    d.insert(
        Name::from("Domain"),
        Object::Array(Array::from(vec![
            Object::Name(Name::from("X")),
            Object::Integer(1),
        ])),
    );
    d.insert(Name::from("N"), Object::Integer(1));
    assert!(parse_function(&Object::Dictionary(d), &direct).is_none());

    // Type 2 with mismatched C0/C1 lengths.
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(2));
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    d.insert(Name::from("N"), Object::Integer(1));
    d.insert(Name::from("C0"), arr(&[0.0, 0.0]));
    d.insert(Name::from("C1"), arr(&[1.0]));
    assert!(parse_function(&Object::Dictionary(d), &direct).is_none());

    // Type 3 stitching with a multi-input domain.
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(3));
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0, 0.0, 1.0]));
    d.insert(Name::from("Encode"), arr(&[0.0, 1.0]));
    d.insert(Name::from("Functions"), Object::Array(Array::from(vec![])));
    assert!(parse_function(&Object::Dictionary(d), &direct).is_none());

    // Type 3 stitching with an /Encode length that does not match /Functions.
    let mut sub = Dictionary::new();
    sub.insert(Name::from("FunctionType"), Object::Integer(2));
    sub.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    sub.insert(Name::from("N"), Object::Integer(1));
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(3));
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    d.insert(
        Name::from("Functions"),
        Object::Array(Array::from(vec![Object::Dictionary(sub)])),
    );
    d.insert(Name::from("Encode"), arr(&[0.0, 1.0, 0.0, 1.0])); // 2 pairs for 1 function
    assert!(parse_function(&Object::Dictionary(d), &direct).is_none());

    // Type 0 with /Size length ≠ /Domain length.
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(0));
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    d.insert(Name::from("Range"), arr(&[0.0, 1.0]));
    d.insert(
        Name::from("Size"),
        Object::Array(Array::from(vec![Object::Integer(2), Object::Integer(2)])),
    );
    d.insert(Name::from("BitsPerSample"), Object::Integer(8));
    assert!(parse_function(&sampled_stream(d, vec![0, 1, 2, 3]), &direct).is_none());

    // Type 0 with an invalid /BitsPerSample.
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(0));
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    d.insert(Name::from("Range"), arr(&[0.0, 1.0]));
    d.insert(
        Name::from("Size"),
        Object::Array(Array::from(vec![Object::Integer(2)])),
    );
    d.insert(Name::from("BitsPerSample"), Object::Integer(7));
    assert!(parse_function(&sampled_stream(d, vec![0, 1]), &direct).is_none());

    // Type 0 with an empty /Range (zero outputs).
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(0));
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    d.insert(Name::from("Range"), Object::Array(Array::from(vec![])));
    d.insert(
        Name::from("Size"),
        Object::Array(Array::from(vec![Object::Integer(2)])),
    );
    d.insert(Name::from("BitsPerSample"), Object::Integer(8));
    assert!(parse_function(&sampled_stream(d, vec![0, 1]), &direct).is_none());

    // Type 0 with an odd-length /Encode.
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(0));
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    d.insert(Name::from("Range"), arr(&[0.0, 1.0]));
    d.insert(
        Name::from("Size"),
        Object::Array(Array::from(vec![Object::Integer(2)])),
    );
    d.insert(Name::from("BitsPerSample"), Object::Integer(8));
    d.insert(Name::from("Encode"), arr(&[0.0, 1.0, 2.0]));
    assert!(parse_function(&sampled_stream(d, vec![0, 1]), &direct).is_none());

    // Type 0 with an /Encode whose pair count ≠ number of inputs.
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(0));
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    d.insert(Name::from("Range"), arr(&[0.0, 1.0]));
    d.insert(
        Name::from("Size"),
        Object::Array(Array::from(vec![Object::Integer(2)])),
    );
    d.insert(Name::from("BitsPerSample"), Object::Integer(8));
    d.insert(Name::from("Encode"), arr(&[0.0, 1.0, 0.0, 1.0]));
    assert!(parse_function(&sampled_stream(d, vec![0, 1]), &direct).is_none());

    // Type 4 with an empty /Range (zero outputs).
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(4));
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    d.insert(Name::from("Range"), Object::Array(Array::from(vec![])));
    assert!(parse_function(&Object::Stream(Stream::new(d, b"{ }".to_vec())), &direct).is_none());

    // Type 0 / Type 4 declared but given a bare dictionary (no stream).
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(0));
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    d.insert(Name::from("Range"), arr(&[0.0, 1.0]));
    d.insert(
        Name::from("Size"),
        Object::Array(Array::from(vec![Object::Integer(2)])),
    );
    d.insert(Name::from("BitsPerSample"), Object::Integer(8));
    assert!(parse_function(&Object::Dictionary(d), &direct).is_none());
}

#[test]
fn malformed_inputs_do_not_panic() {
    // Missing FunctionType.
    let mut d = Dictionary::new();
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    assert!(parse_function(&Object::Dictionary(d), &direct).is_none());

    // Unknown type.
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(9));
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    assert!(parse_function(&Object::Dictionary(d), &direct).is_none());

    // Type 4 with an unknown operator fails to parse.
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(4));
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    d.insert(Name::from("Range"), arr(&[0.0, 1.0]));
    assert!(
        parse_function(
            &Object::Stream(Stream::new(d, b"{ bogusop }".to_vec())),
            &direct
        )
        .is_none()
    );

    // Type 4 stack underflow evaluates to zeros, no panic.
    let mut d = Dictionary::new();
    d.insert(Name::from("FunctionType"), Object::Integer(4));
    d.insert(Name::from("Domain"), arr(&[0.0, 1.0]));
    d.insert(Name::from("Range"), arr(&[0.0, 1.0]));
    let f = parse_function(
        &Object::Stream(Stream::new(d, b"{ add add add }".to_vec())),
        &direct,
    )
    .unwrap();
    approx(&f.eval(&[0.5]), &[0.0]);
}
