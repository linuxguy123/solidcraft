//! Clean-room conversion of saved Part Design operation properties.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FreeCadAxis {
    SketchX,
    SketchY,
    SketchLine { geometry: usize },
    Line { origin: Vec3, dir: Vec3 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FreeCadOperationSettings {
    Extrude { length: f64, length2: Option<f64>, symmetric: bool, reversed: bool, cut: bool, through_all: bool, taper: f64 },
    Revolve { angle: f64, angle2: Option<f64>, symmetric: bool, reversed: bool, cut: bool, axis: FreeCadAxis },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FreeCadOperation {
    pub source_name: String,
    pub label: String,
    pub type_name: String,
    pub profile: String,
    pub base: Option<String>,
    pub body: Option<(String, String)>,
    pub settings: Option<FreeCadOperationSettings>,
    pub warnings: Vec<FreeCadWarning>,
    /// Conditions requiring repair before evaluating this operation.
    pub blockers: Vec<FreeCadWarning>,
}

pub(super) fn capture(object: &mut RawObject, tag: &[u8], property: &str, attrs: &BTreeMap<String, String>) {
    if tag == b"Expression" {
        object.expressions = true;
    }
    if !matches!(
        property,
        "Type"
            | "SideType"
            | "Length"
            | "Length2"
            | "Angle"
            | "Angle2"
            | "TaperAngle"
            | "TaperAngle2"
            | "Reversed"
            | "Midplane"
            | "AlongSketchNormal"
            | "Profile"
            | "BaseFeature"
            | "ReferenceAxis"
            | "Origin"
            | "Suppressed"
    ) {
        return;
    }
    match tag {
        b"Float" | b"Integer" | b"Bool" => {
            object.values.insert(property.into(), string(attrs, "value"));
        }
        b"Link" | b"LinkSub" => {
            object.links.insert(property.into(), (string(attrs, "value"), Vec::new()));
        }
        b"Sub" => {
            if let Some((_, subs)) = object.links.get_mut(property) {
                subs.push(string(attrs, "value"));
            }
        }
        _ => {}
    }
}

fn issue(summary: impl Into<String>, detail: impl Into<String>) -> FreeCadWarning {
    FreeCadWarning { summary: summary.into(), detail: detail.into(), severity: FreeCadWarningSeverity::Issue, context: None }
}

fn value(object: &RawObject, property: &str, default: f64) -> Option<f64> {
    match object.values.get(property) {
        Some(raw) => raw.parse::<f64>().ok().filter(|v| v.is_finite() && v.abs() <= 1e9),
        None => default.is_finite().then_some(default),
    }
}

fn flag(object: &RawObject, property: &str, default: bool) -> bool {
    object.values.get(property).map_or(default, |v| v == "true" || v == "1")
}

fn axis(object: &RawObject, objects: &HashMap<&str, &RawObject>, parents: &HashMap<&str, &str>, profile: &str) -> Option<FreeCadAxis> {
    let (name, subs) = object.links.get("ReferenceAxis")?;
    let sub = subs.first().map(String::as_str).unwrap_or("");
    if name == profile {
        return match sub {
            "H_Axis" => Some(FreeCadAxis::SketchX),
            "V_Axis" => Some(FreeCadAxis::SketchY),
            _ => sub
                .strip_prefix("Edge")
                .and_then(|index| index.parse::<usize>().ok())
                .and_then(|index| index.checked_sub(1))
                .map(|geometry| FreeCadAxis::SketchLine { geometry }),
        };
    }
    let target = objects.get(name.as_str())?;
    if target.type_name == "App::Line" && sub.is_empty() {
        let frame = global_frame(name, objects, parents)?;
        return Some(FreeCadAxis::Line { origin: frame.origin, dir: frame.x });
    }
    None
}

pub(super) fn collect(objects: &[RawObject], by_name: &HashMap<&str, &RawObject>, parents: &HashMap<&str, &str>) -> Vec<FreeCadOperation> {
    objects.iter().filter(|object| (object.type_name.starts_with("PartDesign::") &&
        !matches!(object.type_name.as_str(), "PartDesign::Body" | "PartDesign::Plane" | "PartDesign::Line" | "PartDesign::Point"))
        || matches!(object.type_name.as_str(), "Part::Extrusion" | "Part::Revolution"))
        .map(|object| {
            let profile = object.links.get("Profile").map(|(name, _)| name.clone()).unwrap_or_default();
            let base = object.links.get("BaseFeature").map(|(name, _)| name.clone()).filter(|name| !name.is_empty());
            let mut owner = parents.get(object.source_name.as_str()).copied();
            let mut body = None;
            for _ in 0..64 {
                let Some(name) = owner else { break };
                if let Some(parent) = by_name.get(name) && parent.type_name == "PartDesign::Body" {
                    body = Some((parent.source_name.clone(), parent.label.clone()));
                    break;
                }
                owner = parents.get(name).copied();
            }
            let mut operation = FreeCadOperation {
                source_name: object.source_name.clone(), label: object.label.clone(), type_name: object.type_name.clone(),
                profile, base, body, settings: None, warnings: Vec::new(), blockers: Vec::new(),
            };
            if !matches!(object.type_name.as_str(), "PartDesign::Pad" | "PartDesign::Pocket" | "PartDesign::Revolution" | "PartDesign::Groove") {
                operation.blockers.push(issue("This operation is not supported yet.", format!("FreeCAD operation `{}` has type `{}`. This importer currently rebuilds Pad, Pocket, Revolution and Groove. Operations depending on this feature require repair.", object.source_name, object.type_name)));
                return operation;
            }
            let cut = matches!(object.type_name.as_str(), "PartDesign::Pocket" | "PartDesign::Groove");
            let revolve = matches!(object.type_name.as_str(), "PartDesign::Revolution" | "PartDesign::Groove");
            let mode = value(object, "Type", 0.0).unwrap_or(-1.0);
            let side = value(object, "SideType", if flag(object, "Midplane", false) { 2.0 } else if mode == 4.0 { 1.0 } else { 0.0 }).unwrap_or(-1.0);
            let symmetric = side == 2.0;
            let reversed = flag(object, "Reversed", false);
            let through_all = !revolve && cut && mode == 1.0;
            if flag(object, "Suppressed", false) {
                operation.blockers.push(issue("This operation was suppressed in FreeCAD.", "The imported native feature remains suppressed. Review it before enabling it in SolidCraft."));
            }
            if !(mode == 0.0 || mode == 4.0 || through_all) || ![0.0, 1.0, 2.0].contains(&side) {
                operation.blockers.push(issue("The operation's extent requires manual repair.", format!("FreeCAD extent mode {mode}, side mode {side}. Fixed length/angle, symmetric, two-sided and through-all Pocket are supported. Up-to-face, up-to-first and other reference extents are preserved as suppressed fixed extents using the saved numeric values; edit them before enabling.")));
            }
            if object.links.get("Profile").is_some_and(|(_, subs)| subs.iter().any(|sub| !sub.is_empty())) {
                operation.blockers.push(issue("The selected profile subelements require repair.", "This operation selects individual source profile subelements. The imported feature uses the sketch's complete profiles and is suppressed until its profile selection is repaired."));
            }
            if object.expressions {
                operation.warnings.push(issue("Operation expressions use their saved numeric values.", "FreeCAD formulas and spreadsheet references are not translated yet. The imported operation's numeric dimensions remain editable in SolidCraft."));
            }
            operation.settings = if revolve {
                let angle = value(object, "Angle", f64::NAN);
                let angle2 = if side == 1.0 { value(object, "Angle2", f64::NAN) } else { Some(0.0) };
                match (angle, angle2, axis(object, by_name, parents, &operation.profile)) {
                    (Some(angle), Some(second), Some(axis)) if angle > 0.0 && angle <= 360.0 && second >= 0.0 && angle + second <= 360.0 + 1e-8 =>
                        Some(FreeCadOperationSettings::Revolve { angle, angle2: (side == 1.0).then_some(second), symmetric, reversed, cut, axis }),
                    _ => {
                        operation.blockers.push(issue("The revolution axis or angle could not be imported.", "Supported axes are the profile's horizontal/vertical axis, a line in that sketch, or an origin axis. The angle must be finite and the total sweep at most 360 degrees. No substitute axis or angle is invented."));
                        None
                    }
                }
            } else {
                if !flag(object, "AlongSketchNormal", true) {
                    operation.blockers.push(issue("The custom extrusion direction requires repair.", "The imported feature is suppressed and uses the sketch normal. Edit the sketch plane or feature direction to reproduce the source operation before enabling it."));
                }
                let length = value(object, "Length", if through_all { 1.0 } else { f64::NAN });
                let length2 = if side == 1.0 { value(object, "Length2", f64::NAN) } else { Some(0.0) };
                let taper = value(object, "TaperAngle", 0.0);
                if taper.is_some_and(|angle| angle.abs() > 1e-9) || value(object, "TaperAngle2", 0.0).is_some_and(|angle| angle.abs() > 1e-9) {
                    operation.blockers.push(issue("The tapered extrusion requires a direction check.", "The saved taper angle is retained, but FreeCAD and SolidCraft can use different taper-side conventions. Check the angle and result before enabling this suppressed feature."));
                }
                // FreeCAD accepts a negative one-sided length and extrudes in the
                // opposite direction even when its Reversed property is false.
                let negative_length = mode == 0.0 && side == 0.0 && length.is_some_and(|value| value < 0.0);
                let length = length.map(|value| if negative_length { -value } else { value });
                let reversed = reversed != negative_length;
                match (length, length2, taper) {
                    (Some(first), Some(second), Some(taper)) if side == 1.0 && (first == 0.0 || first == -1.0) && second > 0.0 =>
                        Some(FreeCadOperationSettings::Extrude { length: second, length2: None, symmetric: false, reversed: !reversed, cut, through_all, taper }),
                    (Some(length), Some(second), Some(taper)) if (length > 0.0 || through_all) && second >= 0.0 =>
                        Some(FreeCadOperationSettings::Extrude { length, length2: (side == 1.0).then_some(second), symmetric, reversed, cut, through_all, taper }),
                    _ => {
                        operation.blockers.push(issue("The extrusion dimensions are invalid or unavailable.", "The saved lengths or taper are missing, non-finite or outside SolidCraft's supported range."));
                        None
                    }
                }
            };
            operation
        }).collect()
}
