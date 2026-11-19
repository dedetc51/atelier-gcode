use anyhow::{Result, bail};
use regex::Regex;
use serde::Serialize;
use std::{collections::HashMap, sync::LazyLock};

const EPS: f64 = 1e-5;
static COMMAND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*(?:N\d+\s*)?([GM]\d+(?:\.\d+)?)").unwrap());
static PARAM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)([XYZE])\s*([+-]?(?:\d+(?:\.\d*)?|\.\d+))").unwrap());
static DURATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d+)\s*([dhms])").unwrap());

pub const SETTINGS: &[(&str, &str)] = &[
    ("reduce_crossing_wall", "Éviter de traverser les parois"),
    ("max_travel_detour_distance", "Longueur maximale du détour"),
    (
        "avoid_crossing_wall_includes_support",
        "Inclure les supports dans les détours",
    ),
    ("travel_speed", "Vitesse de déplacement"),
    ("travel_acceleration", "Accélération de déplacement"),
    (
        "reduce_infill_retraction_mode",
        "Rétraction dans le remplissage",
    ),
    ("layer_height", "Hauteur de couche"),
    ("initial_layer_print_height", "Hauteur de première couche"),
    ("wall_loops", "Nombre de parois"),
    ("sparse_infill_density", "Densité de remplissage"),
    ("sparse_infill_pattern", "Motif de remplissage"),
    ("print_sequence", "Séquence d’impression"),
    ("enable_support", "Supports"),
    ("enable_prime_tower", "Tour de purge"),
    ("timelapse_type", "Mode timelapse"),
    ("curr_bed_type", "Type de plateau"),
];

#[derive(Default)]
pub struct Metadata {
    pub config: HashMap<String, String>,
    pub slicer: String,
    pub duration_s: Option<u64>,
    pub objects: Vec<f64>,
    pub filaments: Vec<usize>,
}

impl Metadata {
    pub fn read(gcode: &str) -> Self {
        let mut metadata = Self::default();
        let mut in_config = false;
        let mut in_header = false;
        for line in gcode.lines().filter_map(|line| line.strip_prefix(';')) {
            let tag = line.trim();
            match tag {
                "CONFIG_BLOCK_START" => in_config = true,
                "CONFIG_BLOCK_END" => in_config = false,
                "HEADER_BLOCK_START" => in_header = true,
                "HEADER_BLOCK_END" => in_header = false,
                _ if in_config => {
                    if let Some((key, value)) = tag.split_once('=') {
                        metadata
                            .config
                            .insert(key.trim().into(), value.trim().into());
                    }
                }
                _ if in_header => {
                    if let Some(version) = tag.strip_prefix("BambuStudio ") {
                        metadata.slicer = version.into();
                    } else if let Some((_, time)) = tag.split_once("total estimated time:") {
                        metadata.duration_s = duration_seconds(time);
                    } else if let Some(heights) = tag.strip_prefix("object max height:") {
                        metadata.objects = heights
                            .split(',')
                            .filter_map(|number| number.trim().parse::<f64>().ok())
                            .filter(|number| number.is_finite())
                            .collect();
                        metadata.objects.sort_by(f64::total_cmp);
                    } else if let Some(ids) = tag.strip_prefix("filament:") {
                        metadata.filaments = ids
                            .split(',')
                            .filter_map(|number| number.trim().parse().ok())
                            .collect();
                    }
                }
                _ => {}
            }
        }
        metadata
    }
}

fn duration_seconds(text: &str) -> Option<u64> {
    DURATION
        .captures_iter(text)
        .try_fold(None, |total, capture| {
            let number = capture[1].parse::<u64>().ok()?;
            let multiplier = match &capture[2] {
                "d" => 86400,
                "h" => 3600,
                "m" => 60,
                _ => 1,
            };
            Some(Some(
                number
                    .checked_mul(multiplier)?
                    .checked_add(total.unwrap_or(0))?,
            ))
        })?
}

#[derive(Default, Serialize)]
pub struct Metrics {
    pub trajets_xy: u64,
    pub trajet_mm: f64,
    pub tour_purge_mm: f64,
    pub retractations: u64,
    pub remontees_z: u64,
    pub arcs: u64,
    pub trajets_croisant_paroi: u64,
    pub changements_filament: u64,
}

impl Metrics {
    fn add(&mut self, other: &Self) {
        self.trajets_xy += other.trajets_xy;
        self.trajet_mm += other.trajet_mm;
        self.tour_purge_mm += other.tour_purge_mm;
        self.retractations += other.retractations;
        self.remontees_z += other.remontees_z;
        self.arcs += other.arcs;
        self.trajets_croisant_paroi += other.trajets_croisant_paroi;
        self.changements_filament += other.changements_filament;
    }
}

#[derive(Default, Serialize)]
pub struct Layer {
    couche: usize,
    z_mm: Option<f64>,
    #[serde(flatten)]
    metrics: Metrics,
    trace: Trace,
}

#[derive(Default, Serialize)]
struct Trace {
    wall: Vec<f64>,
    travel: Vec<f64>,
    cross: Vec<f64>,
}

type Point = (f64, f64);
type Segment = (Point, Point);
type WallGrid = HashMap<(i32, i32), Vec<Segment>>;

fn cells((a, b): Segment) -> impl Iterator<Item = (i32, i32)> {
    let index = |value: f64| (value / 10.).floor() as i32;
    (index(a.0.min(b.0))..=index(a.0.max(b.0)))
        .flat_map(move |x| (index(a.1.min(b.1))..=index(a.1.max(b.1))).map(move |y| (x, y)))
}

fn crosses((a, b): Segment, (c, d): Segment) -> bool {
    let side = |(x1, y1): Point, (x2, y2): Point, (x, y): Point| {
        (x2 - x1) * (y - y1) - (y2 - y1) * (x - x1)
    };
    let opposite = |x: f64, y: f64| x.min(y) < -EPS && x.max(y) > EPS;
    opposite(side(a, b, c), side(a, b, d)) && opposite(side(c, d, a), side(c, d, b))
}

fn trace(output: &mut Vec<f64>, (a, b): Segment) {
    output.extend([a.0, a.1, b.0, b.1].map(round_mm));
}

fn round_mm(value: f64) -> f64 {
    (value * 1000.).round() / 1000.
}

pub fn analyze(gcode: &str) -> Result<(Vec<Layer>, Metrics)> {
    let mut layers: Vec<Layer> = Vec::new();
    let (mut layer_active, mut tower, mut toolchange, mut skippable) = (false, false, false, false);
    let (mut relative_xyz, mut relative_e, mut retracted, mut inch) = (false, false, false, false);
    let mut position: [Option<f64>; 4] = [None; 4];
    let mut outer_wall = false;
    let mut walls = WallGrid::new();

    for line in gcode.lines() {
        let (code, tag) = line.split_once(';').unwrap_or((line, ""));
        let tag = tag.trim();
        match tag {
            "CHANGE_LAYER" => {
                layers.push(Layer {
                    couche: layers.len() + 1,
                    ..Default::default()
                });
                layer_active = true;
                walls.clear();
            }
            "MACHINE_START_GCODE_END" => position[..3].fill(None),
            "MACHINE_END_GCODE_START" => break,
            "CP TOOLCHANGE START" => {
                toolchange = true;
                if let Some(layer) = layers.last_mut().filter(|_| layer_active) {
                    layer.metrics.changements_filament += 1;
                }
            }
            "CP TOOLCHANGE END" => toolchange = false,
            "WIPE_TOWER_START" => tower = true,
            "WIPE_TOWER_END" => tower = false,
            "SKIPPABLE_START" => skippable = true,
            "SKIPPABLE_END" => skippable = false,
            _ => {
                if let Some(feature) = tag.strip_prefix("FEATURE:") {
                    let feature = feature.trim();
                    outer_wall = matches!(feature, "Outer wall" | "Overhang wall");
                    if feature == "Custom" {
                        layer_active = false;
                    }
                } else if let Some(height) = tag.strip_prefix("Z_HEIGHT:")
                    && let Some(layer) = layers.last_mut().filter(|_| layer_active)
                {
                    let value = height.trim().replace(',', ".").parse::<f64>()?;
                    if !value.is_finite() {
                        bail!("Hauteur de couche invalide.");
                    }
                    layer.z_mm = Some(value);
                }
            }
        }
        if skippable {
            continue;
        }
        let Some(command) = COMMAND.captures(code).and_then(|match_| match_.get(1)) else {
            continue;
        };
        let command_name = command.as_str().to_ascii_uppercase();
        match command_name.as_str() {
            "G90" => relative_xyz = false,
            "G91" => relative_xyz = true,
            "M82" => relative_e = false,
            "M83" => relative_e = true,
            "G20" => inch = true,
            "G21" => inch = false,
            "G28" | "G150.1" | "G150.3" => position[..3].fill(None),
            "G0" | "G1" | "G2" | "G3" | "G92" => {
                let previous = position;
                let reset = command_name == "G92";
                let mut delta_e = Some(0.);
                for capture in PARAM.captures_iter(&code[command.end()..]) {
                    let axis = match capture[1].as_bytes()[0].to_ascii_uppercase() {
                        b'X' => 0,
                        b'Y' => 1,
                        b'Z' => 2,
                        _ => 3,
                    };
                    let value = capture[2].parse::<f64>()? * if inch { 25.4 } else { 1. };
                    let relative = if axis == 3 { relative_e } else { relative_xyz };
                    position[axis] = if relative && !reset {
                        previous[axis].map(|old| old + value)
                    } else {
                        Some(value)
                    };
                    if !value.is_finite()
                        || (axis < 3 && position[axis].is_some_and(|p| p.abs() > 1000.))
                    {
                        bail!("Coordonnée ou extrusion invalide.");
                    }
                    if axis == 3 {
                        delta_e = if relative_e {
                            Some(value)
                        } else {
                            previous[3].map(|old| value - old)
                        };
                    }
                }
                let Some(extrusion) = delta_e.filter(|_| !reset) else {
                    continue;
                };
                let layer = layers.last_mut().filter(|_| layer_active && !toolchange);
                let starts_retraction = extrusion < -EPS && !retracted;
                if extrusion.abs() > EPS {
                    retracted = extrusion < 0.;
                }
                let Some(layer) = layer else { continue };
                if !tower {
                    layer.metrics.retractations += u64::from(starts_retraction);
                    if let (Some(height), Some(old_z), Some(z)) =
                        (layer.z_mm, previous[2], position[2])
                        && extrusion <= EPS
                        && z > height.max(old_z) + EPS
                    {
                        layer.metrics.remontees_z += 1;
                    }
                }
                if matches!(command_name.as_str(), "G2" | "G3") {
                    layer.metrics.arcs += u64::from(!tower);
                    continue;
                }
                let ([Some(x1), Some(y1), ..], [Some(x2), Some(y2), ..]) = (previous, position)
                else {
                    continue;
                };
                let segment = ((x1, y1), (x2, y2));
                let distance = (x2 - x1).hypot(y2 - y1);
                if distance <= EPS {
                    continue;
                }
                if tower {
                    if extrusion.abs() <= EPS {
                        layer.metrics.tour_purge_mm += distance;
                    }
                } else if extrusion.abs() <= EPS {
                    layer.metrics.trajets_xy += 1;
                    layer.metrics.trajet_mm += distance;
                    if cells(segment)
                        .filter_map(|cell| walls.get(&cell))
                        .flatten()
                        .any(|wall| crosses(segment, *wall))
                    {
                        layer.metrics.trajets_croisant_paroi += 1;
                        trace(&mut layer.trace.cross, segment);
                    } else {
                        trace(&mut layer.trace.travel, segment);
                    }
                } else if outer_wall
                    && extrusion > EPS
                    && matches!((position[2], layer.z_mm), (Some(z), Some(height)) if (z - height).abs() <= 0.001)
                {
                    for cell in cells(segment) {
                        walls.entry(cell).or_default().push(segment);
                    }
                    trace(&mut layer.trace.wall, segment);
                }
            }
            _ => {}
        }
    }

    if layers.is_empty() {
        bail!("Aucune couche Bambu Studio détectée.");
    }
    let mut totals = Metrics::default();
    for layer in &mut layers {
        totals.add(&layer.metrics);
        layer.metrics.trajet_mm = round_mm(layer.metrics.trajet_mm);
        layer.metrics.tour_purge_mm = round_mm(layer.metrics.tour_purge_mm);
    }
    Ok((layers, totals))
}
