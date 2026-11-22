//! The source car: what the physics stages need from any game, in one
//! small representation. A source (AC1 today, `source::ac1`) fills it;
//! the EVO side (tyre fit, suspension, the whole-car check) reads only it.
//!
//! Units: SI (m, kg, N, N/m, N s/m, rad unless named `_deg`), positions
//! relative to the CoG in the source's car frame with x to the left, y up,
//! z forward (AC1's and EVO's convention); suspension hard points relative
//! to the wheel centre at the design position, x positive inboard (AC1's
//! convention, which EVO's geometry messages share).
//!
//! The tyre is a force curve (`TyreCurve`) so the fit never sees a
//! source's parameterisation: AC1 gives its compound model; a game with
//! measured or tabulated curves would implement the trait over its table.

pub mod ac1;

/// One tyre's steady-state force at a load, slip angle and slip ratio.
pub trait TyreCurve: Send + Sync {
    /// (lateral, longitudinal) force in N; `alpha` rad, `sr` negative when
    /// braking, `speed` the road speed in m/s.
    fn force(&self, fz: f64, alpha: f64, sr: f64, speed: f64) -> (f64, f64);
    /// a representative load (the source's reference load)
    fn fz_ref(&self) -> f64;
    /// whether the source fixes the longitudinal slip stiffness (else the
    /// fit matches longitudinal force in slip normalised to the peak)
    fn long_stiffness_known(&self) -> bool {
        true
    }
    /// take the target model's form for what the source leaves undocumented
    fn set_shape(&mut self, _s: &Shape) {}
}

/// The parts of a tyre curve a source may leave undocumented, taken from
/// the EVO compound the fit starts from.
#[derive(Clone, Copy, Debug)]
pub struct Shape {
    /// exponent of the brush curve below the peak
    pub brush_exp: f64,
    /// exponent of the fall past the peak, lateral / longitudinal
    pub post_exp_y: f64,
    pub post_exp_x: f64,
    /// longitudinal over lateral slip stiffness
    pub kx_over_ky: f64,
}

impl Default for Shape {
    fn default() -> Self {
        Shape { brush_exp: 3.0, post_exp_y: 1.0, post_exp_x: 1.0, kx_over_ky: 2.0 }
    }
}

pub struct Tyre {
    pub name: String,
    pub short: String,
    pub width: f64,
    pub radius: f64,
    pub rim_radius: f64,
    /// vertical rate, N/m
    pub rate: f64,
    pub curve: Box<dyn TyreCurve>,
    /// source-specific facts for notes (keys read, laws used)
    pub notes: Vec<String>,
}

/// One compound: front and rear tyre.
pub struct Compound {
    pub tyres: [Tyre; 2],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Geometry {
    /// points in the EVO DoubleWishboneData order: car top front, car top
    /// rear, car bottom front, car bottom rear, tyre top, tyre bottom, car
    /// steer, tyre steer
    DoubleWishbone([[f64; 3]; 8]),
    /// strut car, strut tyre, car bottom front, car bottom rear, tyre
    /// bottom, car steer, tyre steer
    Strut([[f64; 3]; 7]),
    /// links (car side, axle side), attach position, leaf spring lateral
    /// rate, torque reaction
    Axle { links: Vec<([f64; 3], [f64; 3])>, attach: f64, leaf_lat_k: f64, torque_reaction: f64 },
    /// five links (car side, tyre side)
    MultiLink(Vec<([f64; 3], [f64; 3])>),
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Default)]
pub struct Dampers {
    pub bump: f64,
    pub rebound: f64,
    pub fast_bump: f64,
    pub fast_rebound: f64,
    /// m/s where the fast rates start
    pub fast_bump_threshold: f64,
    pub fast_rebound_threshold: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Axle {
    pub geometry: Geometry,
    /// the source's name for the type (notes)
    pub kind: String,
    pub track: f64,
    /// wheel rate, N/m (+ progressive N/m^2)
    pub spring: f64,
    pub progressive: Option<f64>,
    pub dampers: Dampers,
    /// anti-roll bar, N/m at the wheel
    pub arb: f64,
    pub bump_stop_rate: f64,
    /// bump / droop travel to the stops from the design position, m
    pub bump_stop_up: f64,
    pub bump_stop_down: f64,
    /// spring rest length offset (AC1 ROD_LENGTH), m
    pub rod_length: f64,
    /// wheel centre height relative to the CoG at the design position, m
    pub base_y: f64,
    pub hub_mass: f64,
    pub toe_out: f64,
    pub static_camber_deg: f64,
    pub rim_offset: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Engine {
    /// crank torque at full throttle, (rpm, N m)
    pub torque: Vec<(f64, f64)>,
    pub idle: f64,
    pub limiter: f64,
    /// forward ratios, first gear first
    pub gears: Vec<f64>,
    pub final_drive: f64,
    /// 0 RWD, 1 FWD, 2 four-wheel drive
    pub driven: u8,
}

#[derive(Clone, Debug, Default)]
pub struct Aero {
    /// downforce coefficient x area per axle (front, rear), m^2, and drag
    pub cl_area: [f64; 2],
    pub cd_area: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Material {
    pub name: String,
    pub shader: String,
    /// (slot, texture name)
    pub textures: Vec<(String, String)>,
    pub params: Vec<(String, f32)>,
}

pub struct SourceCar {
    /// the source game ("ac1")
    pub game: &'static str,
    pub name: String,
    /// with driver, without fuel
    pub mass: f64,
    pub fuel_kg: f64,
    /// front weight share 0..1
    pub cg_front: f64,
    /// CoG height over the ground at rest, m
    pub cg_height: f64,
    pub wheelbase: f64,
    /// front, rear
    pub axles: [Axle; 2],
    pub compounds: Vec<Compound>,
    pub default_compound: usize,
    pub engine: Engine,
    /// total brake torque N m and the front share 0..1
    pub brake_torque: f64,
    pub brake_front: f64,
    pub aero: Aero,
    pub materials: Vec<Material>,
}

impl SourceCar {
    pub fn mass_static(&self) -> f64 {
        self.mass + self.fuel_kg
    }
    /// static load of one wheel of axle `a` (0 front)
    pub fn wheel_load(&self, a: usize) -> f64 {
        let share = if a == 0 { self.cg_front } else { 1.0 - self.cg_front };
        self.mass_static() * crate::car::physics::G * share / 2.0
    }
}
