// SPDX-License-Identifier: Apache-2.0
//! `vyges-pdn` — power distribution network generation.
//!
//! A design needs power everywhere, delivered as a mesh of wide metal laid over the whole die and
//! stitched together with vias. This engine builds that mesh.
//!
//! - **[`nets`]** — which nets a component builds for, and in what order. Not cosmetic: the order
//!   decides which net gets the innermost ring, so getting it backwards produces a grid that is
//!   geometrically perfect and electrically inverted.
//! - **[`rings`]** — the loop of wide metal around the core, one ring per net, each further out
//!   than the last. Everything else connects *to* it, so a ring that differs makes every later
//!   comparison meaningless.
//! - **[`straps`]** — the repeating stripes that carry power across the die, one per net at each
//!   step of a pitch. The bulk of a grid by area and by shape count.
//! - **[`followpins`]** — the rails along every standard-cell row. Not laid on a pitch of their
//!   own: the rows decide where they go, and they take their width from the cells themselves.
//! - **[`grid`]** — the order the components of a grid are built in, the one retry a component
//!   that produced nothing is given, and the refinement loop. Sequence only, behind a trait, so it
//!   can be tested without any geometry.
//! - **[`shapes`]** — merging shapes and cutting them around obstructions.
//! - **[`vias`]** — where the vias go: finding the candidates and thinning them. The order the
//!   candidates are thrown away in is what decides the answer, because each removal changes what
//!   the next test sees.
//! - **[`viagen`]** — which via to build there. Two preference orders that look alike, are spelled
//!   alike, and run in opposite directions: enclosures pick the smallest, generators the largest.
//! - **[`trim`]** — pulling each shape back to what actually connects to it, and removing what does
//!   not. Runs after the vias and cannot be decided before them.
//!
//! Nothing in this module reads a database; the binary does that and hands values in. That split
//! is what lets the geometry be tested without a design.
pub mod channels;
pub mod followpins;
pub mod grid;
pub mod nets;
pub mod orient;
pub mod pads;
pub mod rings;
pub mod shapes;
pub mod split;
pub mod straps;
pub mod switches;
pub mod techvia;
pub mod trim;
pub mod viagen;
pub mod vias;

/// `(xlo, ylo, xhi, yhi)`, in database units, as odb spells a rectangle.
pub type Rect = (i32, i32, i32, i32);

/// A routing layer's preferred direction, as the technology declares it.
///
/// ⚠️ `None` is a real value and not a missing one — a layer may legitimately declare no preferred
/// direction, and the rules here test *against* `Horizontal` rather than *for* `Vertical`, so an
/// undirected layer follows the same path a vertical one does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Horizontal,
    Vertical,
    None,
}
