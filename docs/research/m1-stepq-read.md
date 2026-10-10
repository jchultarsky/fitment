# stepq 0.5 read additions: M1 proposal

As of 2026-10-09. Status: **accepted** on 2026-10-09, with one change: **PR 6 (PMI to faces) moves to stepq 0.6**, before fitment's M5. M1 is PRs 1–5, and stepq 0.5.0 is published after PR 5. Divergent placements are refused (decision 6), and every other recommendation in section 7 stands.

Six agents each designed one addition against stepq's code, the fixtures and the fitment spike. A separate adversarial reviewer checked each design, and a final agent ordered and unified them. Probe scripts, crafted STEP files and Open CASCADE comparison output are in [m1-evidence/](m1-evidence/), which is not committed. Paths in the text that point to `.../scratchpad/m1/` correspond to `m1-evidence/`.


## 1. Summary

- **Order.** Six read-only PRs, one per addition: units, then geometry values, placements, topology, solids of a definition, and PMI to faces. PR 1 also does three other things:
  - sets `version = "0.5.0"`;
  - rewrites the rule in the four documents;
  - adds a private `src/record.rs` that every reader uses.

  Topology depends on nothing, so it can be reviewed alongside PRs 2 and 3.
- **Shared types.** Points and vectors are `[f64; 3]`, with one `geometry::Rigid` for transforms. There is one unit model, `units::ContextUnits`. It holds factors to mm, rad and sr exactly as the file states them, and reads or refuses each unit kind on its own. The scale is applied once, when a value is read, by a `geometry::Reader`. A reader is tied to a representation or an item and cannot be built from bare factors. Each module has its own typed error. Valid entities that stepq does not read come back as `Other` variants. Walks list what they skipped (`not_followed`, `unresolved`) and offer `is_complete()`.
- **Exit test.** M1's exit test is two Open CASCADE 8.0.1 comparisons: PR 2's surface and circle check, and PR 3's occurrence check. They run in CI on the core fixtures plus generated cases, and once locally on all 55 fixtures. The prototypes already match Open CASCADE:
  - 4,359 of 4,359 occurrences, with |ΔR| at most 2.2e-16 and |Δt| at most 1.1e-13 mm;
  - 342,731 face surfaces read without error, equal to Open CASCADE on 11 files to 2.8e-15.
- **What the reviews changed:**
  - Placements refuse the two cases where ISO and Open CASCADE disagree. The first is an `item_defined_transformation` where neither frame is the identity; I re-checked this one myself. The second is a placement frame that omits `ref_direction` while its axis is not +Z.
  - The PMI resolver becomes a memoised walk over a DAG. Enumerating every route was exponential: a 7 KB file used 5.1 GB.
  - Solids report external definitions. A `split --master` stub read as empty and complete.
  - Topology exposes the composed face sense. An outer `oriented_closed_shell(.F.)` turned a shaft into a hole.
  - Placement frames take their unit from their position in the relationship (ISO 10303-43), not from which representation lists them.
- **Size.** Five PRs are M and one is L. About 55 new public types, no new dependency, MSRV stays 1.85. Output changes only where a silent drop is fixed:
  - `stepq pmi` gains the 6 dimensions it skips on stc_07, plus an opt-in `--geometry`;
  - `stepq tree` places usages it currently drops when they go through complex `MAPPED_ITEM`s. No fixture has one.

## 2. Dependency graph and PR order

```text
(1) units ──► (2) geometry ──► (3) placements ──► (5) solids of a definition
     │                               ▲                     ▲
     └───────────────────────────────┴─────────────────────┘  each frame scaled in its own context
(4) topology      no dependency (reuses PR 1's private container table)
(6) PMI to faces  no dependency (callers convert PMI values with units::unit)
```

| PR | Addition | Needs | Why this position |
| --- | --- | --- | --- |
| 1 | 4. Unit scale, plus 0.5.0 and the rule | none | Every value reader needs a context's factors. It is the first code the old rule forbids, so the rule and the version change with it. |
| 2 | 2. Geometry values | 1 | Defines `Reader`, `Axis2Placement3d` and `Rigid`, which PRs 3 and 5 use. Delivers the cylinder-axis half of the exit test. |
| 3 | 1. Placement transforms | 1, 2 | Delivers the other half of the exit test. It carries the riskiest convention (ARCHITECTURE.md's item_1 rule is wrong for 131 occurrences), so it should come early. |
| 4 | 3. Topology walk | none | Independent; fitment's M2 needs it together with PR 2. Placing it after the exit test keeps it from holding up PRs 2 and 3. |
| 5 | 6. Solids of a definition | 3, 1 | `ShapeItem::transform` composes mapped items using PR 3's frame rules, in each frame's own units. |
| 6 | 5. PMI to faces | none | Fitment needs it last (metadata, D10). It changes `stepq pmi` output, so it follows the purely additive PRs. |

No two additions have to land together.

## 3. Shared foundations (designed once)

**3.1 Layout.**
- New public modules: `stepq::units` (PR 1), `stepq::geometry` (PR 2), `stepq::topology` (PR 4).
- New items in existing modules: `model::Placements` (PR 3), `ProductStructure::shape` (PR 5), `pmi::AspectItems` (PR 6).
- A private `src/record.rs` (PR 1) replaces three copies of the same helpers: `props::{entity, text}`, `assembly::{primary_record, reference, upper}` and `lint::context_of`. It holds:
  - `entity()`: the upper-case type, with complex partial entities joined by `+`, as `props::Subject::entity` does;
  - `partial()` and `exact_params(record, n)`;
  - `real()`: an INTEGER or REAL token, finite;
  - `representation(id) -> (items, context)`: a simple subtype is read from its first three attributes, a complex instance from its REPRESENTATION partial;
  - the container table (which attribute of which shell, solid or model holds what). `units::ContextUnits::of_item` and `topology` both use it, so the two walks cannot drift apart.

`lint` keeps its own loose reader, as the units review asked, so lint findings do not change.

**3.2 Numbers.**
- Points and vectors are `[f64; 3]`: lengths in mm, plane angles in rad, directions of unit length.
- There is one transform type, `geometry::Rigid`:
  - rotation stored row-major, `rotation[row][col]`, the layout of `gp_Trsf::Value(row, col)`;
  - translation in mm;
  - `a * b` applies `b` first;
  - it maps child coordinates into parent coordinates.
- There are no `Point` or `Direction` newtypes. fitment-core converts at its boundary into its own `Point3`, `Dir3` and `Rigid` (workspace-layout §6), so stepq's types never reach fitment's public API.

**3.3 Units, applied in one place.**
- `units::ContextUnits` gives a context's length, plane-angle and solid-angle units, each its own `Result`, plus its uncertainties.
- The only public way to read numbers is a `geometry::Reader`, made with `for_representation` or `for_item`. It converts every length and plane angle it returns. There is no constructor from bare factors and no unscaled path; raw text stays available through `p21::Record`.
- Factors are used as stated: nothing is snapped to 25.4 or π/180.
- A zero length needs no unit, so an origin frame still reads in a context that has no readable length unit.
- Each placement frame is scaled by the context its position assigns (ISO 10303-43):
  - `transform_item_k` uses `rep_k`'s context;
  - `mapping_origin` uses the context of `mapped_representation`;
  - `mapping_target` uses the context of the representation that lists the mapped item.
- Transforms are composed in mm.

**3.4 What stepq does not read: three forms, never a silent default.**
- **`Other { instance, entity }` values:** an entity stepq does not interpret, such as a B-spline surface, an ellipse or any complex geometry instance. The caller treats it as unknown.
- **Typed errors:** a value the caller asked for that cannot be read as stated.
  - One type per module: `UnitError`, `GeometryError`, `PlacementError`, `TopologyError`.
  - Each is `#[non_exhaustive]`, derives `Debug`, `Clone`, `PartialEq` and `thiserror::Error`, and names the innermost `#id` and its entity type.
  - Shared vocabulary:
    - `Unsupported`: valid STEP that stepq does not read;
    - `Malformed`: contradicts the schema (attribute count, type, a WHERE or UNIQUE rule);
    - `Divergent`: stated, but ISO and Open CASCADE read it differently (placements only);
    - a wrapped `UnitError` where a unit is the cause.
  - `stepq::Error` gains one `#[error(transparent)]` variant per module. That is additive, because `Error` is `#[non_exhaustive]`.
  - I did not adopt the topology design's crate-level `Error::Unsupported` and `Error::Malformed`: `Error` holds an `io::Error`, so it is neither `Clone` nor `PartialEq`, and results stored per item (`Occurrence::transform`, `ContextUnits::length`) must be both.
- **Gap lists:** walks that return many things list what they did not follow, in `DefinitionShape::not_followed` and `Resolution::unresolved`. Each entry has `instance`, `entity` and a reason enum, and `is_complete()` is the single switch fitment checks.

**3.5 Strict reading.**
- Every reader checks the exact attribute count of the record or partial it reads.
- A dangling reference reports the instance that holds it (`from`, `to`), not the root of the walk.
- INTEGER tokens are accepted where REALs are expected.
- Non-finite values are refused.
- A direction is normalised by dividing by max|rᵢ| before squaring, so extreme ratios neither overflow nor underflow.

**3.6 Crafted input.**
- Every walk is bounded and reports reaching a bound (`TooDeep`, `TooLarge` or `Cycle`); none truncates silently.
- Walks over DAGs are memoised (PMI, solids).
- The occurrence iterator is lazy and uses memory proportional to depth; fitment caps the number of occurrences.
- One fuzz target, `fuzz/fuzz_targets/read.rs`, runs every reader on every instance of arbitrary bytes. It starts in PR 1, grows in each PR, and joins the 60-second runs in CI's fuzz job.

**3.7 Serde.**
- Result types derive `Serialize` under the `serde` feature.
- Tagged enums use `tag = "kind"` and snake_case, as `model::Placement` does.
- No field of a tagged variant may be called `kind`; the geometry review found that collision. A unit test serialises one value of every result type and fails on duplicate keys.

**3.8 Policy constants.** They are kept in one place and documented, and none refuses anything in the 55 fixtures.

| Policy | Value | Used by | Evidence |
| --- | --- | --- | --- |
| Axis nearly parallel to ref_direction | refused when sin < 1e-6 (ISO refuses only exactly 0) | geometry, placements | smallest sine in the fixtures is 0.99999999999 |
| Cone semi-angle | refused outside [1e-6, π/2 − 1e-6] rad after conversion | geometry | 90 × 0.0174532925 is π/2 − 1.79e-9, which would otherwise read as a valid cone |
| Conversion-unit chain | depth 8 | units | deepest chain in the fixtures: inch → centimetre → SI |
| Assembly depth | 1024 (the existing `MAX_DEPTH`) | placements | deepest fixture assembly: 7 levels |
| PMI route length | 64 | PMI | longest fixture route: 2 steps |
| Shape walk | depth and item budgets, like `MAX_BOM_NODES` | solids | no second hops in the fixtures |

**3.9 Tests and CI.**
- Unit tests sit inline with small Part 21 snippets, as in `pmi.rs` and `props.rs`.
- Fixture tests go in `tests/<module>.rs`. They skip when fixtures are absent, unless `STEPQ_REQUIRE_FIXTURES=1` is set.
  - The CI `occt` job, which already fetches the core set, sets that variable and runs these tests. Pinned counts are then enforced in CI, not only locally.
  - Files over 16 MB are included only with `STEPQ_LARGE_FIXTURES=1`.
- Open CASCADE checks: one function per check in `tools/verify-occt.py`, and one driver, `tools/verify-read.py CHECK` (see §6).
- Inputs that only Open CASCADE can write are generated in the CI job by `tools/gen-occt-cases.py`, with the pinned cadquery-ocp, and never committed. They are: nested assemblies, the same part in five unit systems, a part with a void, and the divergence cases.
- Before each PR: the CLAUDE.md commands plus `cargo +1.85 check --all-features`. Do not use let-chains; they are stable only from Rust 1.88.

## 4. The additions, in PR order

### PR 1: Unit scale (addition 4), version 0.5.0, the rule · effort M

**Module.** In this PR:
- `src/units.rs`, public as `stepq::units`;
- `src/record.rs` (private);
- `version = "0.5.0"`, with a CHANGELOG `[Unreleased]` entry;
- the rule rewording (§5);
- `examples/units.rs`;
- the `read` fuzz target.

```rust
//! stepq::units: unit factors as the file states them. A unit nobody states is an error, never a millimetre.
#[non_exhaustive] pub enum UnitKind { Length, PlaneAngle, SolidAngle }   // canonical: mm, rad, sr

#[non_exhaustive] pub struct Unit {
    pub instance: u64,
    pub kind: UnitKind,
    pub name: String,   // labelled as info::Units labels it: "millimetre", "inch", "degree"
    pub factor: f64,    // value × factor = mm | rad | sr; stated value × base factor; SI prefixes exact
}
#[non_exhaustive] pub struct Uncertainty {
    pub instance: u64, pub name: String, pub description: Option<String>,
    pub value: f64,     // mm (or rad), converted with its OWN unit
    pub unit: Unit,
}
#[non_exhaustive] pub struct ContextUnits { pub context: u64, pub dimension: Option<u32>, /* private: Result per kind */ }
impl ContextUnits {
    pub fn of_context(graph: &Graph<'_>, context: u64) -> Result<Self, UnitError>;
    pub fn of_representation(graph: &Graph<'_>, representation: u64) -> Result<Self, UnitError>;
    /// Every representation that lists `item`, directly or through a shell, solid or surface model,
    /// must resolve. A kind (and the dimension) is Ok only if every holder agrees on it.
    pub fn of_item(graph: &Graph<'_>, item: u64) -> Result<Self, UnitError>;
    pub fn length(&self) -> Result<&Unit, UnitError>;
    pub fn plane_angle(&self) -> Result<&Unit, UnitError>;
    pub fn solid_angle(&self) -> Result<&Unit, UnitError>;
    pub fn uncertainty(&self) -> Result<&[Uncertainty], UnitError>; // Ok(empty) only if none is stated
}
/// Any named unit by #id, simple or complex. Also resolves the units of props and PMI values.
pub fn unit(graph: &Graph<'_>, id: u64) -> Result<Unit, UnitError>;
pub fn representation_context(graph: &Graph<'_>, representation: u64) -> Result<u64, UnitError>;

#[derive(Debug, Clone, PartialEq, thiserror::Error)] #[non_exhaustive]
pub enum UnitError {
    Undefined { from: u64, to: u64 },
    NotAUnit { instance: u64, entity: String },
    Unsupported { instance: u64, entity: String },        // mass, time, derived, context_dependent_unit, SI gram…
    Malformed { instance: u64, reason: &'static str },    // arity, unknown prefix, SI+CBU, `$`/empty/non-reference sets
    NotPositive { instance: u64, value: f64 },            // factor or uncertainty <= 0, NaN, infinite
    KindMismatch { instance: u64, declared: UnitKind, found: UnitKind },
    TooDeep { instance: u64 },                            // chain deeper than 8, or a cycle
    NotAContext { instance: u64, entity: String },
    NotARepresentation { instance: u64, entity: String },
    Missing { context: u64, kind: UnitKind },
    Ambiguous { context: u64, kind: UnitKind, units: Vec<u64> },
    Unreadable { context: u64, unit: u64, reason: Box<UnitError> },
    NotHeld { item: u64 },
    HolderUnreadable { item: u64, holder: u64, reason: Box<UnitError> },
    ContextsDisagree { item: u64, kind: Option<UnitKind>, contexts: [u64; 2] }, // None: the dimension differs
}
```

**Read as stated:**
- **SI units:** all 16 prefixes, from an exact table; simple or complex form.
- **Conversion-based units:**
  - factor = stated value × base unit's factor;
  - the value may be typed or plain, inside any `*_MEASURE_WITH_UNIT`, a plain `MEASURE_WITH_UNIT`, or a complex partial;
  - the name is reported, never interpreted;
  - low-precision degree factors are used as written (six variants occur in the fixtures).
- **Contexts:** `GLOBAL_UNIT_ASSIGNED_CONTEXT` with one unit per kind. Units of other kinds are ignored, and duplicate units with bit-equal factors are accepted.
- **Uncertainties:** read per context, each in its own unit.
- **`coordinate_space_dimension`.**

**Refused:**
- **Per kind:** `Missing`, `Ambiguous` or `Unreadable`, so a broken angle unit blocks only cones. A listed unit whose kind cannot be told refuses every kind.
- **`KindMismatch`:** whenever the kind record, the measure subtype, the typed value's name or explicit dimensional exponents disagree. All 1,330 conversion-based units in the fixtures agree.
- **`Malformed`:** a `$`, empty or non-reference unit set or uncertainty set. It never reads as `Ok(empty)`.
- **`HolderUnreadable` from `of_item`:** a holder that cannot be read fails the call instead of being skipped. Holders are compared kind by kind, dimension included. A walk that grows too large is an error, not a stop.

**Review:** I applied all ten fixes.
- **Mixed-unit count:** corrected to 35 placements outside Olympus plus 7 in it. I re-checked PSU 3KW (4 of 72 RRWTs) and BBU 3KW (2 of 74); both stay under 16 MB, so they run in the default fixture tests.
- **Contract for addition 1:** frames are scaled by position (§3.3).
- **Linking geometry to its context:** I did not adopt the per-graph index. The reader is tied to an item in PR 2 instead. An index over every item would add a second containment model that nobody has measured, and tying the reader to an item already removes the API path that let a wrong context in.

**Tests:**
- **Unit tests:**
  - the design's list: the prefix table; an inch defined as 2.54 × centimetre; a foot defined as 12 × inch; the degree 0.0174532925 kept as written; every error;
  - the review's cases:
    - an uncertainty set written `($)` gives `Malformed`;
    - an inch `TESSELLATED_SHAPE_REPRESENTATION_WITH_ACCURACY_PARAMETERS` as a second holder gives `ContextsDisagree`;
    - a holder with a dangling context gives `HolderUnreadable`;
    - an unreadable uncertainty in two holders still leaves length `Ok`;
    - a 2D and a 3D holder give `ContextsDisagree { kind: None }`;
    - the kind cross-checks.
  - f64 products are compared with `to_bits`.
- **Fixture tests:**
  - every context in the fixtures reads its length and plane-angle units;
  - as1_pe: 7 contexts, 25.4, 1.745329251994E-2, and closures of 0.115, 0.363 and 0.672 mm;
  - nist_ftc_07: an inch defined through the centimetre;
  - as1-ec: 252 parametric contexts, every kind `Missing`;
  - every representation, solid and face resolves (1,348 solids and 342,733 faces with the large files);
  - PSU 3KW and BBU 3KW mixed-unit placements in the default run.
- **Oracle:** `verify-read.py units` (§6). Expected: 1,670 of 1,670 contexts locally, with one length factor 1 ulp off at a relative tolerance of 1e-12; 159 contexts in CI; and the five generated plates (MM, INCH, FT, CM, M).

**Key risk:** uncertainties are per context and depend on the exporter (Pro/E's 'closure' reaches 0.672 mm). Fitment needs its own policy for the tolerance floor (§8).

### PR 2: Geometry values (addition 2) · effort M

**Module.** `src/geometry.rs`, public as `stepq::geometry`. `examples/geometry.rs` walks forward from each representation's items, as the prototype did, until PR 4 lands.

```rust
//! stepq::geometry: geometry the file states (ISO 10303-42), read by #id, in mm and rad.
#[derive(Debug, Clone, Copy, PartialEq)] #[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Rigid { pub rotation: [[f64; 3]; 3], pub translation: [f64; 3] }
impl Rigid {
    pub const IDENTITY: Self;
    pub fn apply_point(&self, p: [f64; 3]) -> [f64; 3];
    pub fn apply_vector(&self, v: [f64; 3]) -> [f64; 3];
    pub fn inverse(&self) -> Self;      // (Rᵀ, −Rᵀt), no re-orthonormalisation
    pub fn is_identity(&self) -> bool;  // exact; #[expect(clippy::float_cmp, reason = …)]
}
impl std::ops::Mul for Rigid { type Output = Rigid; }

pub struct Reader<'g, 'a> { graph: &'g Graph<'a>, units: ContextUnits }
impl<'g, 'a> Reader<'g, 'a> {
    pub fn for_representation(graph: &'g Graph<'a>, representation: u64) -> Result<Self, UnitError>;
    /// Units of every representation holding `item` (a solid, shell or face); they must agree.
    pub fn for_item(graph: &'g Graph<'a>, item: u64) -> Result<Self, UnitError>;
    pub fn units(&self) -> &ContextUnits;
    pub fn point(&self, id: u64) -> Result<[f64; 3], GeometryError>;      // mm
    pub fn direction(&self, id: u64) -> Result<[f64; 3], GeometryError>;  // unit length, never scaled
    pub fn vector(&self, id: u64) -> Result<Vector, GeometryError>;
    pub fn axis1_placement(&self, id: u64) -> Result<Axis1Placement, GeometryError>;
    pub fn axis2_placement_3d(&self, id: u64) -> Result<Axis2Placement3d, GeometryError>;
    pub fn surface(&self, id: u64) -> Result<Surface, GeometryError>;
    pub fn curve(&self, id: u64) -> Result<Curve, GeometryError>;
}

#[non_exhaustive] pub struct Axis2Placement3d {
    pub instance: u64,
    pub location: [f64; 3],                         // mm
    pub x: [f64; 3], pub y: [f64; 3], pub z: [f64; 3], // build_axes, first_proj_axis of AP203e2/AP214e3/AP242e4
    pub axis_stated: bool, pub ref_direction_stated: bool,
}
impl Axis2Placement3d { pub fn to_rigid(&self) -> Rigid; }  // local → enclosing, mm

#[non_exhaustive] pub enum Surface { Plane(Plane), Cylindrical(CylindricalSurface), Conical(ConicalSurface),
    Spherical(SphericalSurface), Toroidal(ToroidalSurface), Other(OtherGeometry) }
#[non_exhaustive] pub enum Curve { Line(Line), Circle(Circle), SurfaceCurve(SurfaceCurve), Other(OtherGeometry) }
#[non_exhaustive] pub struct SurfaceCurve { pub instance: u64, pub subtype: SurfaceCurveKind, // was `kind`: serde clash
    pub curve_3d: Box<Curve>, pub master_representation: MasterRepresentation }
// Each with `instance`: Plane{position}; CylindricalSurface{position, radius}; ConicalSurface{position, radius,
// semi_angle /* rad */}; SphericalSurface{position, radius}; ToroidalSurface{position, major_radius, minor_radius};
// Line{pnt, dir: Vector}; Circle{position, radius}; Vector{orientation, magnitude /* mm */};
// Axis1Placement{location, axis, axis_stated}; OtherGeometry{instance, entity /* '+'-joined if complex */}.

#[derive(Debug, Clone, PartialEq, thiserror::Error)] #[non_exhaustive]
pub struct GeometryError { pub instance: u64, pub entity: String, pub attribute: Option<&'static str>, pub kind: GeometryErrorKind }
#[non_exhaustive] pub enum GeometryErrorKind {
    Missing, WrongType { expected: &'static str }, Complex, ParameterCount { expected: usize, found: usize },
    MissingAttribute, NotANumber, NotAReference, NotAList, NotFinite, Dimension { found: usize },
    ZeroDirection, Parallel, OutOfRange { value: f64 }, UnknownEnumeration, Unit(UnitError),
}
```

**Read as stated:**
- `CARTESIAN_POINT` with 3 coordinates.
- `DIRECTION`, normalised.
- `VECTOR`; its magnitude is a length.
- `AXIS1_PLACEMENT`.
- `AXIS2_PLACEMENT_3D`, through `build_axes`:
  - the default z is (0,0,1);
  - the default x is (1,0,0), or (0,1,0) when z is exactly ±X;
  - a stated `ref_direction` is projected off z.
- `PLANE`, and the cylindrical, conical, spherical and toroidal surfaces.
- `LINE`, and `CIRCLE` with a 3D placement.
- `SURFACE_CURVE`, `SEAM_CURVE` and `INTERSECTION_CURVE`: `curve_3d` is read one level deep, and `master_representation` is reported.

**Reported as `Other`:**
- every other simple type;
- every complex instance (in the fixtures: rational B-splines only);
- a surface curve nested inside a surface curve.

**Errors:**
- the exact parameter count:
  - 2 for `PLANE`, `CARTESIAN_POINT` and `DIRECTION`;
  - 3 for cylinder, sphere, `VECTOR`, `AXIS1_PLACEMENT`, `LINE` and `CIRCLE`;
  - 4 for cone, torus, `AXIS2_PLACEMENT_3D` and the surface-curve family.
- `ZeroDirection` only when every ratio is exactly 0;
- `Parallel` and the semi-angle range per §3.8;
- a radius ≤ 0 (for a cone, < 0);
- 2D points, directions and placements;
- `Unit`, only when a non-zero length or a cone angle needs a unit the context does not provide.

**Review:** I applied all eight fixes.
- `master_representation` is reported, and the `kind` field of `SurfaceCurve` is renamed `subtype`.
- The plane-angle unit is needed only by cones. `ContextUnits`' per-kind results replace the design's `Scale`, which is dropped.
- Corrected text: Open CASCADE clamps a semi-angle ≤ 1e-12 to 1e-12 and drops a cone at or above π/2. It does not accept negative angles.

I disagree with the reviewer on two points:
- **The 90° margin:** the proposed 1e-9 does not cover the 10-digit factor 0.0174532925 (gap 1.79e-9), so I use 1e-6.
- **AP203 edition 1 with z = −X:** six `PLANE` and `CIRCLE` positions (in gear, clevis21 and valve) have an x direction that their own schema leaves indeterminate. I report them with `ref_direction_stated = false` rather than refusing them, because there x only sets the surface's parameterisation. PR 3 refuses the case where x matters.

**Tests:**
- **Unit tests:**
  - (a) to (i) from the design;
  - the serde test for unique keys;
  - extreme ratios: 1e-200, 1e200, and (3e-162, 4e-162, 0);
  - a record with one parameter too many;
  - 90° with the factors 0.0174532925 and 0.01745329251994;
  - AP203 edition 1 with z = −X.
- **Fixture tests:**
  - every face surface and edge curve reachable from a representation reads without error;
  - counts per kind are pinned, for example nist_ctc_04_ap242 with 116 cones, 228 cylinders, 89 planes, 29 tori and 22 spheres;
  - drill points read as 59° in both degree and radian files;
  - nist_ftc_07's cones read as 1° and 2°;
  - clevis21 has `INTERSECTION_CURVE` with master `CURVE_3D`, and as1-ec has `SURFACE_CURVE` with `PCURVE_S2`.
- **Oracle:** `verify-read.py surfaces` and `curves` (§6).

**Key risk:** a wrong context makes every value wrong. Tying the reader to its item and running the oracle on inch, foot, degree and radian files guard against it.

### PR 3: Placement transforms (addition 1) · effort M

**Module.** `src/model/placement.rs`, re-exported from `stepq::model`. This PR also:
- **Fixes the complex `MAPPED_ITEM` attribute offsets** in `src/model/assembly.rs:762` and `:777`. Those lines read `mapping_source` and `mapping_target` at positions 1 and 2, but in a complex instance the partial record holds them at 0 and 1. Today such usages are silently left unplaced.
- **Reads complex representations** in `mapped_items` through `record::representation`.
- **Rewrites ARCHITECTURE.md**, section "Assembly structure" (§5).
- **Updates the docs** of `Placement` and `ProductStructure`.
- **Adds `examples/occurrences.rs`.**

```rust
pub struct Placements<'s> { /* &'s ProductStructure, one Result per usage */ }
impl<'s> Placements<'s> {
    /// Never fails as a whole: each usage has its own result.
    pub fn new(graph: &Graph<'_>, structure: &'s ProductStructure) -> Self;
    /// Component coordinates (mm) → assembly coordinates (mm). Panics if out of range.
    pub fn usage(&self, usage: usize) -> Result<Rigid, PlacementError>;
    /// Every occurrence under `root`, depth first in file order, root first. Lazy, O(depth) memory.
    pub fn occurrences(&self, root: usize) -> Occurrences<'_>;
}
pub struct Occurrences<'p> { /* explicit stack */ }   // Iterator<Item = Occurrence>
#[non_exhaustive] pub struct Occurrence {
    pub path: Vec<usize>,     // usage indices, root first; empty for the root
    pub definition: usize,
    /// The space `transform` maps from: the last usage's component-side representation; for the root, the
    /// one its usages share (its only shape representation if it has no usages); None only if ambiguous.
    pub representation: Option<u64>,
    pub transform: Result<Rigid, PlacementError>,   // occurrence (mm) → root (mm)
}
#[derive(Debug, Clone, PartialEq, thiserror::Error)] #[non_exhaustive]
pub enum PlacementError {
    Unplaced { usage: u64 },
    Quantified { usage: u64, quantity: Option<f64> },       // ≠ 1, or unreadable
    NoTransformation { usage: u64, relationship: u64 },
    Unsupported { usage: u64, instance: u64, entity: String },
    Inconsistent { usage: u64, instance: u64, reason: &'static str },
    Divergent { usage: u64, instance: u64, reason: DivergentReason },
    Frame { usage: u64, error: GeometryError },             // includes a unit missing behind a non-zero location
    Cycle { usage: u64 },
    TooDeep { usage: u64 },
}
#[non_exhaustive] pub enum DivergentReason {
    /// Neither IDT item is the identity: ISO gives M(assembly side)·M(component side)⁻¹,
    /// Open CASCADE 8.0.1 M(item_1)⁻¹·M(item_2).
    BothFramesPlaced,
    /// A placement frame omits ref_direction and its axis is not exactly +Z: the default x of ISO and
    /// Open CASCADE differ (axes −Z, ±X, ±Y, (1,1,1) measured).
    DefaultedRefDirection,
}
```

**Read as stated:**
- Frames are read through PR 2, each in its own context (§3.3).
- **Item-defined transformation:** `transform_item_k` is an item of `rep_k` (AP242 WR2), and the NAUO decides which rep is the component's. The transform is M(assembly-side item)·M(component-side item)⁻¹.
- **Mapped item:** M(target)·M(origin)⁻¹. ISO and Open CASCADE agree here, as the reviewer's `moon-origin.stp` shows.
- **Composition:** transforms compose from the root down each path.
  - Separate roots are unrelated frames.
  - A repeated sub-assembly appears once per path.
  - Mapped items pair with usages in file order. The set of transforms is exact; which usage gets which follows `ProductStructure`'s pairing, as documented.

**Refused, per usage:**
- **`Unplaced`:** no placement, or a different number of mapped items than usages.
- **`Quantified`:** a quantity other than 1, or one that cannot be read.
- **`NoTransformation`.**
- **`Unsupported`:** every `cartesian_transformation_operator`, `functionally_defined_transformation`, `set_item_defined_transformation`, and items that are not an `axis2_placement_3d`.
- **`Inconsistent`:** any of these:
  - `rep_1` and `rep_2` are not exactly one of the component's and one of the assembly's shape representations;
  - `item_k` is not listed directly in `rep_k.items`;
  - the usage has more than one CDSR;
  - a path changes representation within one definition;
  - a root's usages disagree on its representation.
- **`Divergent`.**
- **`Frame`.**
- **`Cycle` and `TooDeep`.**

No call fails as a whole. Fixtures: 3,481 usages (3,327 normal, 131 reversed, 23 mapped), 0 refused, 0 divergent.

**Review:** I applied all five fixes. Defaulted directions flow from PR 2's flags into `Divergent`. The occurrence example prints NAUO ids, names and descriptions for the oracle.

I narrowed one fix. The reviewer would refuse whenever the component-side frame is not the identity; I refuse only when neither frame is. I re-ran Open CASCADE 8.0.1 on a variant where only the component-side frame is non-identity (`comp-nonidentity-asm-identity.step`): PLATE lands at R₁ᵀ, (0, 0, −10), which is M(item_1)⁻¹ in both readings. With both frames non-identity (`both-nonidentity.step`), Open CASCADE gives (0, −10, −10) and ISO gives (0, 0, −20). Both files are under `/private/tmp/claude-501/-Users-julian-projects-fitment/a5c45451-ac4a-479b-8b68-aa1013b0eef5/scratchpad/m1/`, in `proposal/` and `review-placements/`.

**Tests:**
- **Unit tests:**
  - Rigid algebra;
  - the design's cases:
    - a rotated and translated item_2;
    - a reversed SRR;
    - item_1 found only in rep_2 gives `Inconsistent`;
    - a mapped item with an offset target;
    - an inch component whose component-side item is non-identity, under a mm assembly whose item is the identity, scales by 25.4;
    - every refusal;
    - a regression test for the complex `MAPPED_ITEM` offsets;
  - plus:
    - both items non-identity gives `BothFramesPlaced`;
    - axis (0,0,−1) without a ref_direction gives `DefaultedRefDirection`;
    - an unreadable quantity gives `Quantified { None }`;
    - the root's representation is set;
    - a repeated sub-assembly, several roots, a cycle, a chain 1,100 deep, and laziness of the iterator.
- **Fixture tests:**
  - every usage reads (3,481 with the large files);
  - exactly 131 are reversed, in as1_pe, vaccase and weldment;
  - a few matrices from Open CASCADE are pinned (as1-ug nut-and-bolt);
  - PSU 3KW and BBU 3KW are in the default run.
- **Oracle:** `verify-read.py occurrences` (§6).

**Key risk:** the convention itself. ARCHITECTURE.md's "transform_item_1 is the child frame", read literally, misplaces 131 of the 139 occurrences in the Pro/ENGINEER files. The rewrite and the pinned reversed files land in this PR.

### PR 4: Topology walk (addition 3) · effort M

**Module.** `src/topology.rs`, public as `stepq::topology`; plus `examples/topology.rs`.

```rust
//! stepq::topology: B-rep topology of one solid as stated. References and flags only, no numbers.
#[non_exhaustive] pub struct Solid { pub instance: u64, /* shells, faces, edges, vertices, edge-use index */ }
impl Solid {
    pub fn read(graph: &Graph<'_>, id: u64) -> Result<Self, TopologyError>;
    pub fn outer(&self) -> &Shell;  pub fn voids(&self) -> &[Shell];  pub fn shells(&self) -> &[Shell];
    pub fn faces(&self) -> &[Face]; pub fn edges(&self) -> &[Edge];   pub fn vertices(&self) -> &[Vertex];
    /// same_sense composed with the holding shell's orientation (the schema's conditional_reverse DERIVE
    /// of oriented_closed_shell): true if the outward face normal is the surface normal. Use this, not
    /// Face::same_sense, to tell a hole from a shaft — outer shells included.
    pub fn face_sense(&self, face: usize) -> bool;
    pub fn uses(&self, edge: usize) -> &[EdgeUse];
    pub fn is_seam(&self, edge: usize) -> bool;            // ≥ 2 uses, all by one face
    pub fn neighbours(&self, face: usize) -> Vec<usize>;
    pub fn face_vertices(&self, face: usize) -> Vec<usize>;
}
#[non_exhaustive] pub struct Shell { pub instance: u64, pub closed_shell: u64, pub orientation: Option<bool>, pub faces: std::ops::Range<usize> }
#[non_exhaustive] pub struct Face { pub instance: u64, pub shell: usize, pub surface: u64, pub same_sense: bool, pub bounds: Vec<FaceBound> }
#[non_exhaustive] pub struct FaceBound { pub instance: u64, pub outer: bool /* face_outer_bound written; false ≠ inner */, pub bound: Loop, pub orientation: bool }
#[non_exhaustive] pub enum Loop {
    #[non_exhaustive] EdgeLoop { instance: u64, edges: Vec<OrientedEdge> },
    #[non_exhaustive] VertexLoop { instance: u64, vertex: usize },
}
#[non_exhaustive] pub struct OrientedEdge { pub instance: u64, pub edge: usize, pub orientation: bool, pub start: usize, pub end: usize }
#[non_exhaustive] pub struct Edge { pub instance: u64, pub start: usize, pub end: usize, pub curve: u64, pub same_sense: bool } // + is_closed()
#[non_exhaustive] pub struct Vertex { pub instance: u64, pub point: u64 }
/// One use of an edge. `forward` = oriented_edge.orientation == face_bound.orientation: start→end within the face.
#[non_exhaustive] pub struct EdgeUse { pub face: usize, pub bound: usize, pub index: usize, pub forward: bool }

#[derive(Debug, Clone, PartialEq, thiserror::Error)] #[non_exhaustive]
pub enum TopologyError {
    Missing { instance: u64 },                                            // the root #id is not in the file
    UnresolvedReference { from: u64, to: u64 },                           // `from` is the holder, not the solid
    Unsupported { instance: u64, entity: String, expected: &'static str }, // a valid subtype stepq does not read
    Malformed { instance: u64, entity: String, problem: String },        // wrong type, arity, WHERE/UNIQUE, explicit DERIVE
}
```

**Read as stated:**
- `manifold_solid_brep` and `brep_with_voids`;
- `closed_shell`, and `oriented_closed_shell` as the outer shell or a void;
- `advanced_face`, `face_outer_bound`, `face_bound`, `edge_loop`, `vertex_loop`, `oriented_edge`, `edge_curve` and `vertex_point`.

Every flag is exposed exactly as written. Two values beyond the attributes are reading, not computing: the derived vertices of an oriented edge, and the edge-use reverse index. So are the composed `face_sense` and `EdgeUse::forward`, which follow the schema's DERIVE and ISO's definitions.

**Unsupported** (refuses the whole solid). A per-slot table of known subtypes separates this from Malformed. Covered:
- a root that is not a `manifold_solid_brep` or `brep_with_voids` (`shell_based_surface_model`: 87 in the fixtures; also `faceted_brep`);
- `face_surface`, `oriented_face` and `subface`;
- a `vertex` that is not a `vertex_point`;
- any complex topology instance.

**Malformed** (refuses the whole solid):
- the wrong type for a slot;
- `poly_loop` or `subedge` inside an `advanced_face` (its WR7 and WR1);
- a nested `oriented_closed_shell` (WR1);
- a repeated oriented edge in a loop (UNIQUE);
- an empty SET, or a flag other than `.T.` or `.F.`;
- a derived attribute written explicitly;
- two outer bounds on a face, or a face listed twice;
- a void that is not an `oriented_closed_shell`;
- a loop not closed by instance identity (the Creo chassis loop #1200882).

**Not checked** (left to fitment's gate, documented): manifoldness, agreement between geometry and topology, and which loop is outer when no `face_outer_bound` is written.

**Review:** I applied all seven fixes.
- I accept an outer `oriented_closed_shell` instead of refusing it. It is valid, and the composed accessor reads it correctly.
- The guidance for fitment now uses `face_sense` and `forward`.
- The topology design's crate-level errors were not adopted (§3.4).

**Tests:**
- **Unit tests:**
  - one per encoding: the OCCT pin with a seam, Pro/E half faces, Unigraphics' two `face_bound`s, a sphere's `vertex_loop`, and a Creo/OCCT void;
  - the reviewer's `pin-outer-oriented` case, where `face_sense` must read a shaft;
  - one per refusal, checking the `#id` and the class;
  - a missing root, and a deep dangling reference that reports its holder;
  - attribute positions checked against `tests/schemas/*.exp`, as a unit test reading `CARGO_MANIFEST_DIR`.
- **Fixture tests:**
  - pinned totals (solids/faces/edges/vertices): as1 5/53/126/84; as1-md and as1-ug 5/39/70/56; clevis21 1/48/78/62; stc_09 1/125/392/271 with 33 seams;
  - per solid: every edge has 2 uses with opposite `forward`, seams stay within one face, and loops chain;
  - every surface model gives `Unsupported`;
  - `ContextUnits::of_item` resolves every face reached.
- **Oracle:** `verify-read.py topology` (§6).

**Key risk:** one malformed loop rejects a whole solid. For a neighbour in an assembly, that makes the socket incomplete, and fitment must reject on it (§8).

### PR 5: Solids of a definition (addition 6) · effort M

**Module.** `src/model/shape.rs`, re-exported from `stepq::model`; plus `examples/shapes.rs`.

```rust
impl ProductStructure {
    /// The definition's own shape: solids, other items, and every link not followed. Never fails.
    pub fn shape(&self, graph: &Graph<'_>, definition: usize) -> DefinitionShape;
}
#[non_exhaustive] pub struct DefinitionShape { pub solids: Vec<ShapeItem>, pub other: Vec<ShapeItem>, pub not_followed: Vec<NotFollowed> }
impl DefinitionShape { pub fn is_complete(&self) -> bool; }
#[non_exhaustive] pub struct ShapeItem {
    pub item: u64, pub entity: String,
    pub representation: u64,          // lists the item
    pub context: Option<u64>,         // its context_of_items
    pub shape_representation: u64,    // the definition's representation it was reached from (one entry per root)
    pub path: Vec<ShapeLink>,
}
impl ShapeItem {
    pub fn is_transformed(&self) -> bool;
    /// `representation` (mm) → `shape_representation` (mm); each origin read in the mapped representation's
    /// units, each target in the holding representation's; PR 3's Divergent rules apply.
    pub fn transform(&self, graph: &Graph<'_>) -> Result<Rigid, PlacementError>;
}
#[non_exhaustive] pub enum ShapeLink {
    Relationship { relationship: u64, representation: u64 },
    MappedItem { mapped_item: u64, representation_map: u64, origin: u64, target: u64, representation: u64 },
}
#[non_exhaustive] pub struct NotFollowed { pub instance: u64, pub entity: String, pub reason: NotFollowedReason }
#[non_exhaustive] pub enum NotFollowedReason {
    External { document: Option<u64> },       // CAx-IF external definition, SHAPE_REPRESENTATION_REFERENCE, edition-3 reference
    TransformedRelationship, NotShapeRelationship,
    DifferentContext { contexts: [u64; 2] },  // plain SRR between two context instances
    OtherDefinition { definitions: Vec<usize> },
    UnexpectedTarget { expected: &'static str }, // a skip rule's far side is not what the rule requires
    Cycle, TooLarge, Malformed,
}
```

Every public field and variant is documented, since `missing_docs` and `-D warnings` apply. `ShapeLink::MappedItem` uses plain `u64`s and reports an absent origin or target as `Malformed`. It does not claim to match `Placement::MappedItem`; one private function evaluates both.

**Read as stated:**
- the items of each shape representation of the definition;
- a plain SRR in either direction. It must be simple, or complex with exactly the REPRESENTATION_RELATIONSHIP and SHAPE_REPRESENTATION_RELATIONSHIP partials. No CDSR may name it, and both sides must share one context instance;
- a `MAPPED_ITEM` into a representation that no other definition's closure owns;
- as a solid, every instance with a MANIFOLD_SOLID_BREP partial: `BREP_WITH_VOIDS`, `FACETED_BREP` and complex instances, matching the CLI's `is_solid`. Topology refuses `FACETED_BREP`.

**Skipped by rule:**
- `AXIS2_PLACEMENT_3D` items;
- a CDSR-named relationship, only when its far side is a shape representation of that usage's other definition;
- component mapped items that `ProductStructure` matched;
- CGRR and TCGRR, only when `rep_2` is a (tessellated) constructive geometry representation;
- REPRESENTATION_MAPs that point back.

**Reported:**
- every other item goes to `other`, with its type;
- every gap goes to `not_followed`, with one of the reasons above. `External` is reported when:
  - the definition has an `APPLIED_DOCUMENT_REFERENCE`; or
  - a walked representation is tied by a PROPERTY_DEFINITION_REPRESENTATION to a property naming a `DOCUMENT_FILE` or called 'external definition'.

**Review:** I applied all eight fixes and chose between the reviewers' options in two places.
- **`FACETED_BREP`:** listed under `solids`, so there is one definition of "solid", and the oracle equation becomes "Open CASCADE solids = stepq solids + TESSELLATED_SOLID".
- **Duplicates:** one entry per root rather than a list of roots, because the path differs per root.

**Tests:**
- **Unit tests:** the design's 18 tests, with test 10 now expecting one entry per root and test 6 expecting `FACETED_BREP` among the solids. Plus:
  - a `split --master` stub (a placement-only representation plus 'external definition') gives `External`;
  - an SRR across two contexts gives `DifferentContext`;
  - an illegal CGRR gives `UnexpectedTarget`;
  - the reviewer's `mapped_component_body.step` gives `OtherDefinition`;
  - a complex SRR with an extra partial gives `NotShapeRelationship`.
- **Fixture tests:**
  - every definition is complete;
  - per file, the distinct solid `#id`s equal the count of MSB-partial instances (1,261 overall);
  - AS1 for each of its 5 exporters;
  - NIST CTC-02 e2, FTC-08-tg and FTC-09;
  - the moon buggy;
  - the SRR case on AS1 ug and ac (the Creo SRR check stays local);
  - `split --master` output of as1-ug: every stub reads `External`.
- **Oracle:** `verify-read.py shapes` (§6).

**Key risk:** a neighbour's body that stepq cannot reach is a false positive for fitment unless fitment treats `is_complete() == false` as an incomplete socket. The helper reports every gap, but acting on it is fitment's job.

### PR 6: PMI to faces (addition 5) · effort L

**Module.** `src/pmi/items.rs`, declared with `mod items;` in `src/pmi.rs` and re-exported from `stepq::pmi`. This PR also:
- extends `pmi()`: `DIMENSIONAL_SIZE_WITH_DATUM_FEATURE` joins SIZES; `DIRECTED_DIMENSIONAL_LOCATION` and `DIMENSIONAL_LOCATION_WITH_DATUM_FEATURE` join LOCATIONS;
- fixes `props::subject()`. Today it picks the first partial whose name contains SHAPE_ASPECT, which in `COMPOSITE_SHAPE_ASPECT+DATUM_FEATURE+SHAPE_ASPECT` is the empty `COMPOSITE_SHAPE_ASPECT()`. That loses the product for 36 targets today, and for DSWDF and `CENTRE_OF_SYMMETRY` targets;
- adds `stepq pmi --geometry` (table, JSON and CSV), with the docs in `docs/COMMANDS.md`.

```rust
pub struct AspectItems { /* owns its data */ }
impl AspectItems {
    /// One pass over every IIRU and every shape_aspect_relationship (all subtypes, complex too). The aspect
    /// graph is resolved as a DAG: each aspect expanded once, memoised, cycles found by DFS colouring. O(V + E).
    pub fn new(graph: &Graph<'_>) -> Self;
    pub fn usages(&self) -> &[ItemUsage];
    pub fn resolve(&self, graph: &Graph<'_>, definition: u64) -> Resolution;
    /// Definitions whose resolution lists `item`, ascending, with how each reaches it.
    pub fn naming(&self, item: u64) -> &[Naming];
}
#[non_exhaustive] pub struct Resolution { pub definition: u64, pub items: Vec<ResolvedItem>, pub unresolved: Vec<Unresolved> }
impl Resolution {
    pub fn is_complete(&self) -> bool;
    /// Items reachable by a route using only `allowed` kinds (direct items always included).
    pub fn item_ids(&self, allowed: StepKinds) -> Vec<u64>;
}
#[non_exhaustive] pub struct ResolvedItem {
    pub item: u64, pub entity: String,
    pub usage: u64,                    // the first usage naming it
    pub via: Vec<Step>,                // one shortest route
    pub route_kinds: Vec<StepKinds>,   // every distinct set of kinds a route uses (≤ 32); the empty set = direct
}
#[non_exhaustive] pub struct Naming { pub definition: u64, pub route_kinds: Vec<StepKinds> }  // + is_direct()
#[non_exhaustive] pub enum StepKind { Component, DerivedFrom, DatumFeature, DatumTarget, TargetFeature }
pub struct StepKinds(u8);  // a set of StepKind; serialises as a list
#[non_exhaustive] pub struct ItemUsage { pub instance: u64, pub kind: ItemUsageKind, pub entity: String, pub name: Option<String>,
    pub description: Option<String>, pub definition: u64, pub used_representation: Option<u64>, pub items: Vec<u64> }
#[non_exhaustive] pub enum ItemUsageKind { GeometricItemSpecific, ItemIdentified, Presentation /* every DMIA subtype */, Other }
#[non_exhaustive] pub struct Step { pub relationship: u64, pub kind: StepKind, pub aspect: u64 }
#[non_exhaustive] pub struct Unresolved { pub aspect: u64, pub entity: String, pub via: Vec<Step>, pub reason: UnresolvedReason }
#[non_exhaustive] pub enum UnresolvedReason {
    NoUsage, WholeShape,
    NotFollowed { relationship: u64, related: u64 },                  // plain SAR whose meaning only a string gives
    UnsupportedRelationship { relationship: u64, entity: String },    // any other SAR subtype or complex SAR from here
    UnsupportedUsage { usage: u64 },                                  // chain-based, complex, malformed identified_item
    UnsupportedDefinition,                                            // a dimensional_size/location as tolerance target
    ComponentPath, MissingItem { usage: u64, item: u64 }, Missing, Cycle, TooDeep,
}
```

**Read as stated:**
- simple GISU and plain IIRU, including typed `SET_`/`LIST_REPRESENTATION_ITEM` members in order;
- Component: from a composite to its components;
- DerivedFrom: `shape_aspect_deriving_relationship`;
- DatumFeature and DatumTarget, followed backwards from the datum (datum_feature WR1);
- TargetFeature: `feature_for_datum_target_relationship`;
- presentation links, listed but never followed.

**Reported, with a reason:**
- every relationship no rule consumes, from the aspect being resolved. Excluded are the `dimensional_location` family, which are dimensions, and the feature or target basis into a DATUM. Covered:
  - unknown SAR subtypes;
  - complex SARs;
  - plain SARs into a DATUM from anything that is not a datum feature or datum target;
- the remaining reasons listed above.

**Not done** (documented):
- WR1 is not checked: 510 of the 2,587 identified items lie outside `used_representation`;
- stepq does not say which face of a group a value measures;
- `property_definition` → `shape_definition_representation` aspect shapes are not followed.

**Review:** I applied all nine fixes. For a dimension used as a tolerance target I chose `UnsupportedDefinition` over following `applies_to`. None occurs in the fixtures, and following it can become a new `StepKind` later without breaking anything.

**Tests:**
- **Unit tests:**
  - the design's 1 to 12;
  - the reviewer's diamond file (22 layers, 7 KB) must finish in milliseconds with each item listed once;
  - a complex SAR and a `feature_component_relationship` from a composite give `UnsupportedRelationship`;
  - `DMIA_WITH_EXTERNAL_IMAGE_PLACEMENT` is classed as `Presentation`;
  - a dimension as a tolerance target gives `UnsupportedDefinition`;
  - the `item_ids` filter and `naming`'s `route_kinds`;
  - `subject()` on complex, derived and DSWDF aspects.
- **Fixture tests:**
  - the design's pinned counts: 635, 42 and 4 of 681 ends; datums 83, 7, 22 and 2;
  - its spot checks;
  - every NIST tolerance and dimension target has a product definition;
  - `stepq pmi` output is unchanged on every fixture except stc_07, which gains exactly #14028, #14030, #16317, #16368, #16419 and #16470. The CHANGELOG notes this.
- **Oracle:** `verify-read.py pmi` (§6).

**Key risk:** any plain SAR from a composite is read as a component; all 2,174 such relationships in the fixtures agree with Open CASCADE. Separately, a value stated on a group must never be assigned to each face, so fitment has to join through `route_kinds` and `is_direct()`.

## 5. The rule rewording

**One rule, worded the same everywhere:**

> stepq reads what a file states and never computes what it does not. A point, a direction, a radius, an axis, a placement and a unit factor are stated. Reading them may normalise a direction, apply the stated unit factor of the value's own context, and compose stated placements. Intersections, projections, trimmed areas, arc lengths, volumes, bounding boxes, tessellation and healing are computed, and they stay out: use a kernel. A value stepq cannot read with certainty is reported, as an "other" variant or an error with a reason. It is never guessed, never defaulted beyond the schema's own defaults, and never skipped. Reading never changes output: unchanged entities are still written byte for byte.

The four documents change in PR 1, except the "Assembly structure" bullet, which changes in PR 3.

**README.md**
- **Introduction.** Current: "`stepq` works on the **entity graph** of a STEP file, not on its geometry. It never tessellates, never computes a volume, never "heals" anything."
  New: "`stepq` works on the **entity graph** of a STEP file. It reads the geometry a file states — placements, axes, radii, unit factors — and never computes what the file does not: it never tessellates, never computes a volume, never "heals" anything."
- **"What it will not do".** Current: "Anything geometric. No tessellation, no mass properties, no bounding boxes, no unit *conversion*, no shape healing, no format conversion. If you need those, you need a geometry kernel; …"
  New: "Compute geometry. `stepq` reads what a file states — a point, an axis, a radius, a placement, a unit factor — and applying a stated unit factor or composing stated placements counts as reading. It never computes what the file does not state: no intersections, no trimmed areas, no tessellation, no mass properties, no bounding boxes, no shape healing, no format conversion. If you need those, you need a geometry kernel; …" (the rest unchanged).
- **Release PR:** "Status: 0.4, early." becomes 0.5, and the `stepq = { version = "0.4", … }` snippet becomes 0.5.

**CLAUDE.md** (hard rules)
- Current: "No geometry evaluation, ever. If a change needs a curve or surface value, it does not belong here."
- New: "Read what the file states; never compute what it does not. Reading a point, direction, radius, axis, placement or unit factor is in scope, as are normalising a direction, applying the stated unit factor of the value's own context and composing stated placements. Anything that evaluates a curve or surface (intersection, projection, trimmed area, volume, tessellation, healing) does not belong here." Also add: "Never guess: a value that cannot be read with certainty is an `Other` variant or an error with a reason, never a default or a silent skip."
- The OCCT bullet gains: "…and every reader change must keep its `tools/verify-read.py` check green on the core fixtures."
- The pre-PR command list gains `cargo +1.85 check --all-features`.

**docs/ARCHITECTURE.md**
- **One-paragraph summary.** Current: "No entity that describes geometry is ever interpreted; it is copied verbatim."
  New: "Writing never interprets an entity: geometry is copied verbatim. Reading may interpret what a file states, never what it does not (see "Reading stated geometry")."
- **"Assembly structure"** (PR 3). Current: "`transform_item_1` is the child frame, `transform_item_2` the parent frame. The transform is `M(item_2) · M(item_1)⁻¹`; every exporter we have seen writes `item_1` as the identity."
  New: "`transform_item_k` is an item of `rep_k` (AP242 WR2 of `representation_relationship_with_transformation`), and the NAUO, not the relationship's order, decides which of `rep_1`/`rep_2` is the component's. The transform carries the component's frame onto the assembly's: `M(assembly-side item) · M(component-side item)⁻¹` — `M(item_2)·M(item_1)⁻¹` in the usual order, `M(item_1)·M(item_2)⁻¹` in the 131 reversed Pro/ENGINEER usages of the fixtures. The component-side item is the identity in all 3,458 fixture usages. Where neither item is the identity, Open CASCADE 8.0.1 composes the other way round, so stepq refuses such a usage (`PlacementError::Divergent`)."
- **New section, "Reading stated geometry".** PR 1 adds it and each later PR appends to it. It covers:
  - the rule;
  - units per context and the positional rule for placement frames;
  - `build_axes` and the schema edition it follows;
  - the reporting pattern;
  - the known differences from Open CASCADE:
    - the default x direction;
    - an IDT with both frames non-identity;
    - non-identity `AXIS2_PLACEMENT_3D` items in a representation, which Open CASCADE applies as a transform;
    - cone semi-angles that Open CASCADE clamps or drops.
- **"Non-goals, and why".** Current: "Tessellation, mass properties, bounding boxes, healing and unit conversion all require evaluating curves and surfaces."
  New: "Tessellation, intersections, trimmed areas, mass properties, bounding boxes and healing all require evaluating curves and surfaces." (The rest of the paragraph is unchanged.) Add: "Applying a unit factor the file states is reading, not conversion, and is in scope from 0.5.0."

**ROADMAP.md**
- **Introduction.** Current: "Nothing here needs a geometry kernel; that is the constraint that keeps the project finishable."
  New: "Nothing here needs a geometry kernel: stepq reads what a file states and never computes what it does not. That constraint keeps the project finishable." Also add "0.5.0 adds the read milestone" to the version list.
- **New section "## Read (0.5.0)"**, with one checkbox per addition: units, geometry values, placements and occurrences, topology, solids of a definition, PMI to faces.
- **"Not planned".** Current: "Tessellation, mass properties, bounding boxes, unit conversion, healing, any conversion to or from another format. Use a kernel."
  New: "Tessellation, intersections, trimmed areas, mass properties, bounding boxes, healing, any conversion to or from another format. Use a kernel."

**Same PR, outside the four documents:**
- CONTRIBUTING.md: "**No geometry.** Anything that needs to evaluate a curve or surface is out of scope, permanently." becomes "**Stated, not computed.** stepq reads values a file states (points, axes, radii, placements, unit factors) and never evaluates a curve or surface; that is out of scope, permanently."
- `src/lib.rs`: "It never evaluates geometry: there is no tessellation, no volume, no healing." becomes "It reads the geometry a file states — placements, axes, radii, unit factors — but never evaluates it: there is no tessellation, no volume, no healing." The module table gains one row per new module.
- `tools/verify-occt.py` docstring: "stepq never evaluates geometry" becomes "stepq reads only what files state".
- PR 3: `model::Placement`'s "Identified only; no transformation is evaluated." becomes "Identified here; [`Placements`] evaluates them."

## 6. The oracle

**Structure.**
- **Open CASCADE side.** `tools/verify-occt.py` gains one importable function and subcommand per check: `units`, `surfaces`, `curves`, `occurrences`, `topology`, `shapes` and `gdt`. Each prints TSV or JSON. `summary` and `compare` are unchanged.
- **stepq side.** `examples/{units,geometry,occurrences,topology,shapes}.rs`, built with `--release --features serde`, and `stepq pmi --geometry --format json` for PMI. No CLI command is added just for the oracle, so no output format is committed to semver.
- **Driver.** `tools/verify-read.py CHECK [FILE…]` imports `verify-occt.py`, as `verify-split.py` already does. It builds once, runs both sides, joins them, and exits 0, 1 or 2 like `verify-occt.py`.
- **Joins by entity number.** Open CASCADE's entity number equals the file position, which is stepq's node + 1. Every such join asserts that the entity types match. The pybind `Number` and `IdentLabel` return 0, so the lookup uses object identity of `model.Value(i)`.
- **Generated inputs.** `tools/gen-occt-cases.py` writes:
  - a nested assembly with a repeated sub-assembly and arbitrary rotations;
  - one plate in MM, INCH, FT, CM and M;
  - a part with a void and a cone point;
  - two divergence cases, made by editing the text of a generated assembly: one with both IDT frames non-identity, and one with a placement frame on axis (0,0,−1) that omits `ref_direction`.
- **CI `occt` job, after `verify-split.py`:**
  1. generate the cases;
  2. run `verify-read.py` for each check on the core and generated files;
  3. run `STEPQ_REQUIRE_FIXTURES=1 cargo test --all-features --test units --test geometry --test placements --test topology --test shape --test fixtures`.

**Exit test, part 1: placements (`occurrences`).**
- **Open CASCADE side:**
  - walks XCAF from `GetFreeShapes` through `GetComponents(label, False)` to `GetReferredShape`;
  - composes `GetLocation` with `TopLoc_Location.Multiplied`, in mm;
  - keys each occurrence by the instance-name path of its component labels and the product-name path;
  - drops shape-expansion labels (SOLID, SHELL, COMPOUND, '?').
- **stepq side:** prints, per occurrence:
  - the NAUO `#id` path;
  - the NAUO id, name and description path;
  - the product-name path;
  - either the 3×4 row-major matrix in mm or `error: <variant>`.
- **Matching:**
  - exact and one-to-one by instance-name path wherever those names are unique (as1-ug's nut-and-bolt instances NBA1, NBA2 and NBA3 are told apart);
  - by value only for unnamed components, such as the moon buggy's 29 mapped-item occurrences.
- **Fails on any of:**
  - an occurrence left unmatched on either side;
  - a stepq refusal on a fixture;
  - |ΔR| > 1e-12, or |Δt| > 1e-9 · max(1 mm, |t|).
- **Generated divergence cases** must read `Divergent`, with Open CASCADE's value printed for the record. Any other outcome fails.
- **Expected:**
  - 4,359 of 4,359 occurrences on all 55 fixtures (measured 2.2e-16 and 1.1e-13 mm), 0 refused;
  - the OCCT-written nested assembly 8 of 8;
  - CI's own count is pinned when PR 3 lands.

**Exit test, part 2: cylinder axes (`surfaces`, `curves`).**
- **What is compared:**
  - Open CASCADE reads the XCAF part prototypes, unplaced, and takes `BRepAdaptor_Surface` per face.
  - Each surface is reduced to a key that does not depend on x:
    - plane: signed normal (the stated z) and offset;
    - cylinder: axis line as a foot point plus an unsigned direction, and radius;
    - cone: apex, signed opening axis and semi-angle;
    - sphere: centre and radius;
    - torus: centre, unsigned axis and both radii.
  - Circles (centre, signed normal, radius) and lines are read through the surface-curve unwrapping, with `BRepAdaptor_Curve`. This comparison is required, because it is the only cross-check of the path to the circles at the ends of fitment's holes.
- **Pass criteria:**
  - equal counts per kind;
  - stepq's `Other` count equals Open CASCADE's non-elementary count;
  - a one-to-one multiset match at 1e-9 relative.
- **Expected:** 11 files match one to one with a worst deviation of 2.8e-15; signed keys change nothing, as the reviewer measured. Out-of-range cones are covered only by unit tests, since Open CASCADE clamps or drops them.

**Other checks:**
- `units`: factors and tolerance per context (§4, PR 1).
- `topology`, per solid:
  - equal face sets;
  - Open CASCADE's REVERSED equals `!face_sense`;
  - equal sets of faces using each edge Open CASCADE kept;
  - seams are closed in Open CASCADE;
  - each edge's first vertex by parameter equals `edge_start` when `same_sense` is true, else `edge_end`;
  - each use is FORWARD exactly when `forward == same_sense`;
  - a stepq solid with no Open CASCADE counterpart fails;
  - edges replaced by healing are counted but do not fail.
- `shapes`: Open CASCADE's per-label solid counts equal stepq's subtree solids plus `TESSELLATED_SOLID`s.
- `pmi` (STEPCAFControl in GDT mode):
  - tolerances paired one to one by (kind, magnitude, item set);
  - the union of items per datum label must be equal;
  - dimensions must be equal after removing links only stepq makes, which `route_kinds` identify;
  - any superset fails unless it is allowlisted (ftc_10 datums K and L, which reach surface-model faces).

**Fixtures.**
- **In CI:** the core set plus the generated cases.
  - STEP Tools AS1 from 5 exporters.
  - as1_pe, vaccase and weldment: reversed Pro/E placements, in inches.
  - The moon buggy: mapped items, in inches.
  - clevis21, valve and gear: ACIS intersection curves.
  - The NIST PMI set: degree and radian contexts, and an inch defined through the centimetre.
- **Locally, with the result attached to PRs 2 and 3:** all 55 with `STEPQ_LARGE_FIXTURES=1`. This adds:
  - foot contexts (the OCP rack);
  - inch components in mm assemblies (PSU 3KW, BBU 3KW, both shelves, Olympus);
  - depth 7 (Olympus 1U);
  - sub-assemblies reached by several paths.
- **M1 is done when** both checks report 0 mismatches and 0 unexpected refusals on CI's set and on the recorded run of all 55, and the four documents state the rule.

## 7. Decisions for the owner

1. **Order and granularity.** Recommended: six PRs as in §2, with PR 4 optionally reviewed alongside PRs 2 and 3.
2. **Rule wording.** Recommended: the §5 text, landing in PR 1.
3. **Canonical units.** Recommended: mm, rad and sr, applied when values are read, by readers tied to a representation or item. No unscaled reader and no reader built from bare factors.
4. **Error model.** Recommended: one typed error per module plus transparent variants on `stepq::Error` (§3.4), rather than crate-level `Unsupported` and `Malformed`.
5. **Number types.** Recommended: bare `[f64; 3]` and one `Rigid`; no Point or Direction newtypes.
6. **Where ISO and Open CASCADE disagree on a placement.** Recommended: refuse with `PlacementError::Divergent`, in two cases: both IDT frames non-identity, and a placement frame without `ref_direction` whose axis is not +Z. The alternative is to read per ISO and expose a flag.
7. **Thresholds.** Recommended: refuse sin(axis, ref) < 1e-6, and semi-angles outside [1e-6, π/2 − 1e-6] rad. The alternative is ISO's exact-zero rule with no margin.
8. **Strictness of placement structure.** Recommended: strict:
   - `item_k` listed directly in `rep_k`;
   - one CDSR per usage;
   - one representation per definition along a path.

   Relax to WR2's transitive membership when a real file needs it.
9. **Transformation operators.** Recommended: `Unsupported`, including the rigid subset of `cartesian_transformation_operator_3d`, plus functionally and set-defined transformations, until a real file contains one (0 in 55).
10. **Topology.**
    - Recommended: refuse the whole solid when a loop is not closed by instance identity (Creo chassis #1200882).
    - Recommended: accept an outer `oriented_closed_shell`, read through the composed accessors.
11. **Solids.**
    - Recommended: list `FACETED_BREP` as a solid; topology then refuses it.
    - Recommended: do not follow a transformed SRR or a plain RR outside a usage (Open CASCADE reads both as empty).
    - Recommended: one entry per root.
12. **PMI.** Recommended:
    - do not follow the plain 'datum feature' relationship (22 datums; Open CASCADE links nothing there either);
    - include DerivedFrom items, marked by `route_kinds`;
    - report a dimension used as a tolerance target as `UnsupportedDefinition`.
13. **CLI.** Recommended: add only `pmi --geometry`. The other readers reach the oracle through `examples/`. `tree --placements` can come later.
14. **Fixtures.** Recommended:
    - generate the Open CASCADE cases in CI and do not commit them;
    - add the step-book Open CASCADE examples to the fetched core set, with pinned hashes;
    - keep OCP local, with a recorded all-55 run on PRs 2 and 3.

    The alternative is a small PSU/BBU fixture set in CI for the mixed-unit cases.
15. **Release.** Recommended: publish 0.5.0 after PR 6 (fitment needs it before M4), and change the README's version text in that release PR.

## 8. What fitment needs from M1 that this does not cover

**Covered: the spike's gaps.**
- `frame()`: NaN for an x axis without `ref_direction`, and silent `(0,0,1)`/`(1,0,0)` fallbacks. Replaced by `Reader::axis2_placement_3d`.
- One level of placements, with mapped-item and unplaced usages read as the identity, and reversal and units ignored. Replaced by `Placements::occurrences`.
- `features()`: took the solids listed directly in each shape representation, and skipped `brep_with_voids` and faces that were not an `advanced_face`. Replaced by `ProductStructure::shape` with `Solid::read`.
- `face_vertices()`, a forward walk. Replaced by `Solid::face_vertices`, `uses` and `neighbours`.
- No units. Replaced by `units` and readers that return mm and rad.

**Not covered: fitment's own work, or follow-ups.**
- **Feature arithmetic, which is computing and belongs in fitment-core:**
  - grouping coaxial faces by value;
  - arc sweeps and the full-circle test;
  - axial extents;
  - plane outlines and which loop is outer;
  - counterbores and countersinks;
  - the cone apex.
- **Checks for the canonical gate (D2)** that stepq reports but does not enforce:
  - each edge used twice, with opposite `forward`;
  - identity `AXIS2_PLACEMENT_3D` items in each solid's representation (the Open CASCADE frame quirk);
  - whether to trust `curve_3d` under master `PCURVE_S1`/`S2` (fitment's Open CASCADE-written corpus);
  - `Other` surfaces or curves on an interface;
  - `ref_direction_stated == false` where x matters.
- **Turning gaps into refusals on the assembly side.** Any of the following on a neighbour must make the socket incomplete, never be skipped:
  - a refused `Solid::read`;
  - `is_complete() == false`;
  - body-like `other` items;
  - a refused occurrence transform.
- **Policies:**
  - the tolerance floor taken from uncertainties (per context, with names and sizes that vary by exporter);
  - a cap on the number of occurrences, refusing rather than truncating.
- **Master files** read as `External`. Fitment must run `stepq::assemble` first, or refuse.
- **Threads:** thread designations appear only as presentation text (DMIA → callout). There is no helper; fitment joins through `usages()` (D10).
- **Coverage gaps that stay false negatives:** Creo's B-spline arcs, CADDS's B-spline cylinders, ellipses, tessellated solids and surface models.
- **Fitment's own documents:** PLAN.md's "Changes to stepq" row 1 and its "Reference" bullet repeat the old item_1 convention. Row 4 says "the file's" uncertainty, but uncertainty is per context. Both should be corrected after PR 3.
- **Release:** publishing stepq 0.5.0 is a release step after PR 6. The conversion from stepq's arrays and `Rigid` to fitment-core's `Point3`, `Dir3` and `Rigid` is fitment's adapter.

**Evidence I re-checked for this proposal** (under `/private/tmp/claude-501/-Users-julian-projects-fitment/a5c45451-ac4a-479b-8b68-aa1013b0eef5/scratchpad/m1/`):
- `review-placements/both-nonidentity.step` and `proposal/comp-nonidentity-asm-identity.step`: Open CASCADE 8.0.1 run on both; results as in PR 3.
- `placements/ax2_default.py`: the default-x differences, re-run.
- `proposal/mixed.py`: the PSU 3KW and BBU 3KW mixed-unit counts.
- From the stepq sources and schemas:
  - `src/props.rs:255`, the `subject()` partial-record bug;
  - `src/model/assembly.rs:762` and `:777`, the mapped-item offsets;
  - the "Master files" section of `docs/ARCHITECTURE.md`, which describes the external stubs;
  - `tests/schemas/ap242e4_mim_lf.exp`: `oriented_closed_shell` is a subtype of `closed_shell`, `faceted_brep` a subtype of `manifold_solid_brep`;
  - `.github/workflows/ci.yml`: the `occt` job fetches only the core set, the `msrv` job checks with 1.85, and the fuzz job runs only lex and parse.