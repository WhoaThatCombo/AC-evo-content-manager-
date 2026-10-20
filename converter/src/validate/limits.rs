//! Limits the validator holds converted tracks to, measured on a stock
//! content.kspkg by `aceconv stockstats` (`facts.limits` in
//! stockstats\stockstats.json). Stock content is known to load, so staying
//! within it is safe; going past it is not known to fail, which is why the
//! track total is only a warning.

/// ISMesh instances in one node: the largest stock node (cota, `grass_hi`).
pub const ISMESH_NODE_MAX: usize = 1_130_335;
/// ISMesh instances in one track (main scene + containers): the largest
/// stock track (nurburgring).
pub const ISMESH_TRACK_MAX: usize = 4_432_375;
