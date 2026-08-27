#![no_main]
//! Fuzz target for the PDF function parser and evaluator (EPIC 6, ISO 32000 §7.10).
//!
//! Functions are the parameterisation behind `Separation`/`DeviceN` tint transforms (§8.6.6),
//! shading gradients (§8.7.4) and transfer curves. All four flavours parse untrusted dictionary
//! entries, and type 3 (stitching, §7.10.4) *recurses* through `/Functions`, whose entries are
//! indirect references — so a hostile file can point a subfunction back at its own parent. On ANY
//! input, parsing and evaluation must return without panicking, hanging, or exhausting the stack
//! (DESIGN.md §3.4, §7); a stack overflow aborts the process rather than unwinding, so the nesting
//! bound this exercises is load-bearing for the `pdf-ffi` no-unwind contract.
//!
//! Driving it through a whole [`Document`] is deliberate: the resolver is what closes a reference
//! cycle, and `parse_function` on a bare dictionary can never reach that case.
//!
//! Run with: `cargo +nightly fuzz run function` (needs `cargo install cargo-fuzz`).

use libfuzzer_sys::fuzz_target;
use prismpdf::cos::{Object, ObjectId};
use prismpdf::Document;

/// Object numbers probed per input. Well above any fuzzer-reachable document, small enough that one
/// input cannot dominate the campaign.
const MAX_OBJECTS: u32 = 64;

fuzz_target!(|data: &[u8]| {
    let Ok(doc) = Document::open(data.to_vec()) else {
        return;
    };

    for number in 0..MAX_OBJECTS {
        let reference = Object::Reference(ObjectId::new(number, 0));

        // Both entry points a caller has: the raw function parser and the Separation/DeviceN
        // colour space that wraps one as its tint transform.
        if let Some(function) = prismpdf::parse_function(&doc, &reference) {
            // Evaluate at the domain edges and an interior point, with a deliberately wrong input
            // arity in the mix — inputs short of /Domain are padded, extras ignored.
            for input in [&[][..], &[0.0][..], &[0.5, 1.0][..], &[-1e300, 1e300][..]] {
                let _ = function.eval(input);
            }
        }

        if let Ok(Some(separation)) = prismpdf::resolve_separation(&doc, &reference) {
            let _ = separation.to_alternate(&[0.5]);
        }
    }
});
