# Glossary

The canonical vocabulary of the domain — the words that appear in code, tests, comments, and the
spec-map — so the same concept is never named two different ways. Each entry gives the term, what
it means, and the names *not* to use for it.

For what the project is, read `README.md`; for how it is built, `DESIGN.md`.

**Object** (COS object):
One of the nine PDF primitive values defined in ISO 32000 §7.3 — Boolean, Integer, Real,
Name, String, Array, Dictionary, Stream, Null. A *direct* value carrying no identity.
_Avoid_: token, node, element.

**Reference** (indirect reference):
A pointer to an indirect object, written `n g R` in PDF syntax. Modelled as an `Object`
variant carrying an `ObjectId`. A Reference is dumb — it knows *which* object it points to,
not *what* that object is.
_Avoid_: pointer, link, ref-object.

**ObjectId**:
The identity of an indirect object: an object number plus a generation number. Two indirect
objects with the same number but different generations are distinct.
_Avoid_: handle (handle is the FFI concept), key, address.

**Document**:
The owner of an opened PDF — it holds the byte source and the cross-reference table, and is
the **only** thing that can resolve a Reference into the Object it points to. COS objects
never resolve themselves.
_Avoid_: file, reader, store, context.

**Builder** (low-level builder):
An assembly of pages, resources, metadata, and document structures used to create a PDF directly.
It sits below Composition and accepts content operators whose placement is already decided.
_Avoid_: Document, composition builder, writer.

**Resolution**:
Turning a Reference into the Object it points to, by asking the Document. May trigger lazy
parsing of bytes not yet read. COS cannot resolve; only the Document can.
_Avoid_: dereferencing, lookup, fetch.

**Composition**:
An owned description of a new document as pages containing trees of layout elements. Composition
owns page design and pagination; it is distinct from an opened Document and a low-level Builder.
_Avoid_: rendering, HTML layout, flow version 2.

**Page** (composition page):
A page design within a Composition, comprising optional header and footer regions and one content
region. It drives pagination but is not itself a layout element.
_Avoid_: canvas, sheet, page element.

**Layout element**:
One semantic or visual unit in a Composition tree. A layout element is measured against available
space and may contain other layout elements. It is not automatically a tagged-PDF structure
element; layout and document semantics are separate concerns.
_Avoid_: COS Object, structure element, widget, component.

**Container**:
A layout element that owns and arranges child layout elements. Tree order determines logical
reading order even when placement is two-dimensional.
_Avoid_: wrapper, group, structure element.

**Constraint**:
The finite width and height a parent offers a layout element during measurement. A constraint is
available space, not a final position or a demand that the element consume all of it.
_Avoid_: bounds, rectangle, allocation.

**Measurement**:
The deterministic plan a layout element produces for a constraint: empty, complete, partial, or
deferred to fresh space, together with the space it would consume.
_Avoid_: rendering, drawing, placement.

**Placement**:
The assignment of measured space to a position in the composition coordinate system. Placement
must agree with the immediately preceding measurement.
_Avoid_: measurement, rendering, PDF coordinates.

**Semantic tag**:
An accessibility meaning attached to a layout element, such as paragraph, heading, table, link, or
figure. It contributes to tagged-PDF structure but does not determine visual layout.
_Avoid_: layout element, structure element, style, role.
