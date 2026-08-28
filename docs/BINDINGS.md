# Binding author's guide

Language bindings (.NET, Python, Node, Go, Swift, Java, WASM, …) live in their own repositories
and consume this repo's C ABI (`pdf-ffi`) — see `AGENTS.md`, "Language bindings are out of
scope". This guide is the starting point for anyone building such a binding. Its goal is that
every binding **starts the same way and exposes the same API shape**, so `doc.PageCount` in C#,
`doc.page_count` in Python and `doc.pageCount` in JavaScript are recognisably the same call, and
knowledge (docs, examples, tests) transfers across languages.

This guide does **not** restate the ABI contract. The authorities, in order:

1. `crates/pdf-ffi/include/prismpdf.h` — the generated header; each function's doc comment carries
   its exact ownership contract (what is owned, borrowed, or consumed).
2. [`docs/ABI.md`](ABI.md) — status codes, structured failure diagnostics, the Rust→C type and
   ownership tables, collection conventions, versioning policy.

When this guide and those disagree, those win — fix this guide.

## What a binding links against

- **Download the native libraries you ship** from the per-tag bundle described in
  [`native-artifacts.md`](native-artifacts.md): one prebuilt library per platform, built with the
  portability settings (static CRT, glibc floor, separate musl build, macOS deployment target and
  signature) that a binding cannot fix after the fact. Verify the published SHA-256 and package the
  binaries *inside* whatever you ship — never fetch at a consumer's runtime. Do not cross-compile
  this repository yourself.
- **Apple platforms are the exception to "link a shared library".** iOS, iPadOS and Mac Catalyst
  link the engine into the signed application from `PrismPDF.xcframework`; macOS is included too,
  so an Apple binding can use one binary target.
  The framework carries the header and module map for `import CPrismPDF`. This is aimed at an Apple
  binding written in Swift, where static linking is the native idiom. A binding in a managed runtime
  can also consume it, but it pays for it: entry points resolve against the app binary rather than
  a library name (`DllImport("__Internal")` in .NET), so the package has to multi-target — one code
  path bound to a library name, one to the statically linked symbols.
- Build the native library from source with `cargo build -p prismpdf-ffi --release` in a checkout of
  this repo when you are working on the engine or on a platform the matrix does not cover. The
  artifact is **`pdf_ffi`**: `libpdf_ffi.so` (Linux), `libpdf_ffi.dylib` (macOS), `pdf_ffi.dll`
  (Windows). A static `libpdf_ffi.a` / `pdf_ffi.lib` is also produced. Do not confuse it with
  `prismpdf`, which is the CLI binary.
- Vendor (copy and commit) the header `crates/pdf-ffi/include/prismpdf.h` into the binding repo,
  recording the Prism PDF version it came from. The ABI is **append-only** (`docs/ABI.md`,
  "Versioning policy"): existing signatures and status-code values never change, so a binding
  built against an older header keeps working against a newer library. New surface is found by
  diffing the vendored header against the new release's header; ABI additions are recorded in
  `CHANGELOG.md`.
- There are no cargo features to choose; `pdf-ffi` builds one way. (Its only feature,
  `c-acceptance`, compiles this repo's own C tests and is irrelevant to a binding.)

## Architecture: two layers, always

Every binding has exactly two layers:

1. **The raw layer** (`NativeMethods`, `_prismpdf`, `sys`, …): one flat, mechanical, 1:1
   projection of `prismpdf.h`. No logic, no renaming beyond the language's FFI syntax. Because
   the header is cbindgen-generated and uniform, this layer can be hand-written or generated —
   but it must be *complete for every area the binding ships*, and its completeness check is the
   analogue of `crates/pdf-ffi/tests/c/header_surface.c`: every export the binding claims to
   cover is referenced at least once.
2. **The idiomatic layer**: the canonical object model below. This is the public API and the
   only thing user code sees. It is where the ownership and error conventions are enforced, once,
   so user code cannot leak, double-free, or misread a status.

## The canonical object model

One class per **owned** opaque handle; the class name is the handle name minus the `PrismPdf`
prefix. The header's `typedef struct PrismPdf… PrismPdf…;` block is the authoritative inventory;
as of this writing the owned handles are:

| Class | Constructed by | Freed / consumed by |
|-------|----------------|---------------------|
| `Document` | `document_open`, `_with_password`, `_with_options`, `_with_limits`, `_with_private_key` | `document_free` |
| `OpenOptions`, `OpenReport`, `TransformReport` | `*_new` / produced by `*_report` calls | `*_free` |
| `Object`, `Edit` | `object_new_*`, `edit_new` | `object_free`; `edit_commit` **consumes** on success, else `edit_free` |
| `Builder`, `PageSpec`, `Content`, `StructNode` | `*_new` | `builder_build` returns bytes; page/struct-child commits **consume** the committed handle on success |
| `Flow`, `TextBlock`, `Table`, `ImageSource` | `*_new`, `image_source_from_*` | `flow_build` / `flow_into_builder` **consume** the flow; the rest via `*_free` |
| `Composition`, `CompositionContainer` | `composition_new`, `composition_add_page`, container setters | `composition_build` **consumes**; containers via `composition_container_free` |
| `SignSettings`, `XmpMetadata` | `*_new` | `*_free` |
| `AnnotationList`, `FormFieldList`, `OutlineList`, `AttachmentList`, `FontList`, `ImageList`, `SignatureList`, `StringList` | read-side producers on `Document` | `*_list_free` |
| `ErrorInfo` | `last_error` | `error_info_free` — internal to the error path, never public API |

The seven **borrowed** item types (`Annotation`, `FormField`, `OutlineItem`, `Attachment`,
`Font`, `Image`, `Signature`) become lightweight non-owning wrappers whose instances keep their
parent list alive (see "Semantic contracts" below). In the idiomatic layer, prefer exposing each
list as the language's native read-only sequence of item wrappers.

**Do not invent nouns the ABI does not have.** There is no `Page` handle: page-indexed calls
(`prismpdf_page_text(doc, index, …)`, `prismpdf_page_annotations`, …) are methods on `Document`
taking a page index. A binding that grows a `Page` façade diverges from every other binding for
zero capability gain.

## Name derivation rules

The mapping from C name to idiomatic name is mechanical. Apply these rules in order; keep the
words identical and only adapt casing (`PascalCase` / `snake_case` / `camelCase`) and property
syntax to the language.

1. Strip the `prismpdf_` prefix.
2. **The receiver is the first handle parameter, not the name prefix.** `prismpdf_page_text` takes
   a `const PrismPdfDocument *` first, so it is `Document.pageText(index)` — the `page_` prefix
   names the subject, not a receiver type.
3. `<noun>_new*` → constructor (or named constructor for variants, e.g.
   `ImageSource.fromJpeg(...)` from `image_source_from_jpeg`). `document_open*` → static
   factories on `Document`: `Document.open(bytes)`, `Document.open(bytes, password)`,
   `Document.open(bytes, options)`.
4. `<noun>_free` and `*_list_free` → the language's disposal idiom (`IDisposable`/`using`,
   context manager, `Symbol.dispose`, `defer … Close()`), never a public `free` method.
   Deterministic disposal is the API; a finalizer, if the language has one, is only a safety net.
5. Getters with no arguments beyond the receiver become properties where the language has them
   (`document_page_count` → `doc.PageCount`). A getter returning a boolean out-param returns a
   plain boolean.
6. Functions that take **no handle parameter at all** are statics on a top-level class named for
   the library, subject to the language's naming constraints (`PrismPdf.version()`,
   `PrismPdf.merge(...)` — `merge` takes an *array* of documents, not a receiver). Where the
   language cannot spell `PrismPdf` — C#, for instance, cannot give a type the same name as its
   enclosing namespace without ambiguity at every use site — pick the nearest name and record the
   deviation; the placement is the rule, the spelling is not.

   Rule 2 wins wherever the two could disagree: **the signature decides, never the name**.
   `prismpdf_measure_text` and `prismpdf_wrap_text` read like module-level functions and carry no
   `<noun>_` prefix, but both take a `PrismPdfTextBlock *` first, so they are
   `block.measureText(text)` and `block.wrapText(text, width)` — not statics.
7. `*_report` variants: expose the plain call, plus a `…WithReport` companion returning the
   result together with the report object — do not fold the two into one signature with an
   optional parameter, so the cheap path stays report-free in every language.
8. The permission helpers (`prismpdf_permissions_*`) operate on a plain `int32` `/P` value, not a
   handle: bind them as an immutable value type with chainable methods —
   `Permissions.restricted().allowPrint()` — over `permissions_restricted()` +
   `permissions_allow_print(p)`.
9. `#[repr(C)]` enums keep their variant names minus the `PrismPdf…_` prefix, as the language's
   native enum. Never renumber; the C values are the contract.
10. String-pair inputs (two parallel `const char *const *` arrays plus a count, e.g. `fill_form`)
    become the language's native map/dictionary or pair-sequence type.

## Semantic contracts every binding must honor

These are the behaviours that make bindings interchangeable. The underlying rules are in
`docs/ABI.md`; what follows is how they surface in a garbage-collected language.

1. **One error type.** Any status other than `Ok` (and other than `NotFound` on an optional
   getter) raises/returns a single `PrismPdfError` carrying the stable integer status and the
   message from `prismpdf_last_error`. Read the diagnostic **immediately, on the same thread as
   the failed call**, before any scheduler hop (`await`, goroutine switch, thread-pool
   continuation) — the slot is thread-local. Remember that pure argument-check rejections do not
   refresh the slot (`docs/ABI.md`, "Structured failure diagnostics"): populate the message only
   when the snapshot's status matches the failed call, else fall back to the status name.
2. **`NotFound` on an optional getter is absence, not an error.** Map it to the language's
   absence idiom (`null`, `None`, `Optional.empty`). `NotFound` from an index lookup
   (page out of range) is still an error.
3. **Consuming calls come in three shapes — read the export's doc comment, not this rule alone.**
   Getting this wrong is a double free, not a compile error, so the header states the shape for
   every export that takes a handle it may claim. The three, and what a wrapper must do:

   | Marker in the doc comment | Exports | What the wrapper does |
   |---|---|---|
   | **Consumes on success** | `edit_commit`, `builder_add_page_spec`, `builder_add_structure_node`, `struct_node_add_child` | Mark the handle dead **after** `Ok`. A failure leaves it caller-owned and still freeable. |
   | **Consumes always** | `flow_build`, `flow_into_builder` | Mark the handle dead **before** the call. The box is taken as the call is entered, so a failing call has already freed it and a wrapper that frees again on the error path double-frees. |
   | **Finalises** | `composition_build` | Nothing. The handle becomes immutable on success *and* failure — later mutation or build calls return `InvalidUse` — but it is still the caller's to free. |

   Use after a consuming success is the language's "object disposed" error, raised by the wrapper,
   not by the native library.
4. **Borrowed items must keep their owner alive.** An item wrapper from `*_list_get`, and any
   byte-payload view from it, holds a strong reference to the list wrapper so the collector
   cannot free the list while an item is reachable. Never pass a borrowed pointer to any
   `*_free`.
5. **Copy, then free, immediately.** Owned out-strings are copied into a native string and
   released with `string_free`; owned byte buffers are copied and released with `bytes_free`
   passing back the original length — both in a `finally`-equivalent. The single exception:
   `prismpdf_version` returns a static string that is never freed. Never release library memory
   with the language's or the C runtime's allocator.
6. **No shared mutable handles across threads.** The ABI makes no thread-safety promise for a
   handle; treat every handle as externally synchronized (confine it to one thread or guard it
   with a lock). The thread-local diagnostic slot is the visible consequence.

## The conformance suite

A binding's test suite is a port of the same journeys, against the same inputs, asserting the
same facts — that is what "the binding works" means, and it is also how the binding *tests the
SDK* from a real consumer's seat. The journeys are:

- **Parse**: open files from this repo's `corpus/{valid,malformed,edge}` — published
  per tag as `prism-pdf-corpus-vX.Y.Z.tar.gz`, so this needs no checkout; assert
  page counts, versions, extracted text; malformed files must open via recovery or fail with
  `Parse`, never crash.
- **Create**: `Builder` and content streams → bytes → *reopen with the binding
  itself* and assert. Prefer asserting through the SDK's own read API over golden-byte
  comparisons.
- **Manipulate**: merge, extract pages, rotate, fill and flatten forms,
  round-trip save.
- **Compose**: the layout `Flow` and the declarative `Composition`. The anchor
  test is a port of `crates/pdf-ffi/tests/c/compose_invoice.c` — the standalone C consumer that
  builds the tagged acceptance invoice via the composition API. Every binding builds the same
  invoice and asserts on it by reopening.
- **Security**: encrypt/decrypt round-trip, permissions, and — where the binding ships them —
  signing and verification.
- **Failure paths**: wrong password → `Password` error with a message;
  absent optional field → absence, no error; a conformance refusal surfaces its
  `ConformanceIssue`; a disposed/consumed handle raises the wrapper's error, not a crash.

Null-argument and double-free defence is already proven on the Rust side
(`crates/pdf-ffi/src/api/null_sweep.rs`); bindings need not re-sweep it, because their idiomatic
layer makes those states unrepresentable.

## The vertical slice

Every new binding starts with the same slice, in this order, because it exercises every
convention once (status codes, last-error, owned handle, owned string, owned bytes,
failure path):

```text
PrismPdf.version()                                 -> "0.4.1"        (static string)
doc = Document.open(readFile("corpus/valid/…"))   (owned handle; disposal frees it)
Document.open(garbage)                            -> PrismPdfError(status=Parse, message=…)
Document.open(encrypted, password="wrong")        -> PrismPdfError(status=Password, …)
doc.pageCount                                     -> n              (plain value out-param)
doc.pageText(0)                                   -> "…"            (owned string, copied+freed)
doc.pageText(n + 1)                               -> PrismPdfError(status=NotFound, …)
bytes = doc.save()                                (owned buffer, copied+freed)
doc2 = Document.open(bytes); doc2.pageCount == n  (round-trip through the binding itself)
```

Underneath, the raw signatures for the slice (from `prismpdf.h`):

```c
PrismPdfStatus prismpdf_document_open(const uint8_t *data, uintptr_t len, PrismPdfDocument **out_doc);
PrismPdfStatus prismpdf_document_open_with_password(const uint8_t *data, uintptr_t len,
                                                  const uint8_t *password, uintptr_t password_len,
                                                  PrismPdfDocument **out_doc);
PrismPdfStatus prismpdf_document_page_count(const PrismPdfDocument *doc, uintptr_t *out_count);
PrismPdfStatus prismpdf_page_text(const PrismPdfDocument *doc, uintptr_t index, char **out_text);
PrismPdfStatus prismpdf_document_save(const PrismPdfDocument *doc, uint8_t **out_data, uintptr_t *out_len);
```

After the slice, expand area by area in the order of the conformance suite (parse →
create/manipulate → compose → security), and only then decide whether to generate the remaining
raw layer or keep hand-writing it — the slice settles the idioms generation must reproduce.

## New-binding checklist

1. New repository; vendor `prismpdf.h` with its source version recorded; wire a build step that
   fetches and checksum-verifies the published `pdf_ffi` libraries for each target platform
   ([`native-artifacts.md`](native-artifacts.md)), keeping a from-source path for engine work.
2. Raw layer for the vertical slice; safe layer implementing the six semantic contracts.
3. The vertical slice's tests green, including both failure paths.
4. The invoice acceptance port green.
5. Journey suites per shipped area; the raw-layer completeness check; CI on the same OS matrix
   the native library supports.
6. Feed anything this guide got wrong or left ambiguous back into this file.
