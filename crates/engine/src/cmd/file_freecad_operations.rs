//! Rebuild supported FreeCAD history in dependency order, with explicit body targets.
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde_json::{Value, json};
use solidcraft_doc::{AxisRef, BodyName, Direction, Extent, ExtrudeConvention, FeatureKind, Operation, RevolveConvention, SketchId};
use solidcraft_geom::{Vec2, Vec3};
use solidcraft_io::{FreeCadAxis, FreeCadOperation, FreeCadOperationSettings, FreeCadWarning};

use crate::{Result, Session, params::bad};

pub(super) struct ImportedSketch {
    pub id: u64,
    pub component: u64,
    pub body: Option<(String, String)>,
    pub curves: Vec<Option<String>>,
    pub needs_repair: bool,
}

pub(super) fn plan(operations: &[FreeCadOperation], params: &Value) -> Result<Vec<FreeCadOperation>> {
    let cmd = "file.import_freecad_sketches";
    let by_name: BTreeMap<&str, &FreeCadOperation> = operations.iter().map(|op| (op.source_name.as_str(), op)).collect();
    let mut pending: Vec<String> = match params.get("operations") {
        None => operations.iter().map(|op| op.source_name.clone()).collect(),
        Some(Value::Array(values)) if values.len() <= 50_000 => values
            .iter()
            .map(|v| v.as_str().map(str::to_string).ok_or_else(|| bad(cmd, "`operations` must list source operation names")))
            .collect::<Result<_>>()?,
        Some(_) => return Err(bad(cmd, "`operations` must list source operation names")),
    };
    let mut selected = BTreeSet::new();
    while let Some(name) = pending.pop() {
        let operation = by_name.get(name.as_str()).ok_or_else(|| bad(cmd, format!("unknown FreeCAD operation `{name}`")))?;
        if selected.insert(name)
            && let Some(base) = &operation.base
            && by_name.contains_key(base.as_str())
        {
            pending.push(base.clone());
        }
    }
    let mut indegrees = BTreeMap::new();
    let mut children = BTreeMap::<&str, Vec<&str>>::new();
    for name in &selected {
        let op = by_name.get(name.as_str()).ok_or_else(|| bad(cmd, "invalid operation dependency"))?;
        let degree = usize::from(op.base.as_ref().is_some_and(|base| selected.contains(base)));
        indegrees.insert(name.as_str(), degree);
        if let Some(base) = op.base.as_deref().filter(|base| selected.contains(*base)) {
            children.entry(base).or_default().push(name);
        }
    }
    let mut ready: VecDeque<&str> = indegrees.iter().filter_map(|(&name, &count)| (count == 0).then_some(name)).collect();
    let mut ordered = Vec::new();
    while let Some(name) = ready.pop_front() {
        if let Some(op) = by_name.get(name) {
            ordered.push((*op).clone());
        }
        for child in children.get(name).into_iter().flatten() {
            if let Some(count) = indegrees.get_mut(child) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    ready.push_back(child);
                }
            }
        }
    }
    if ordered.len() != selected.len() {
        return Err(bad(cmd, "the FreeCAD operation history contains a dependency cycle"));
    }
    Ok(ordered)
}

fn warning(summary: &str, detail: impl Into<String>) -> FreeCadWarning {
    FreeCadWarning { summary: summary.into(), detail: detail.into(), context: None }
}

fn axis(s: &Session, sketch: &ImportedSketch, source: &FreeCadAxis) -> std::result::Result<(AxisRef, Vec3, Vec3), String> {
    let state = s.model.state();
    let solved = state.sketch(sketch.id).ok_or("The profile sketch could not be evaluated.")?;
    let plane = solved.plane;
    Ok(match source {
        FreeCadAxis::SketchX => (AxisRef::SketchAxis { axis: "x".into() }, plane.origin, plane.x),
        FreeCadAxis::SketchY => (AxisRef::SketchAxis { axis: "y".into() }, plane.origin, plane.y),
        FreeCadAxis::SketchLine { geometry } => {
            let curve = sketch.curves.get(*geometry).and_then(Option::as_ref).ok_or("The source axis geometry was not imported.")?;
            let index = solved.sketch.curve_index(curve).ok_or("The axis curve no longer exists.")?;
            let Some(solidcraft_sketch::CurveKind::Line { a, b }) = solved.sketch.curves.get(index).map(|curve| &curve.kind) else {
                return Err("The axis geometry is not a line.".into());
            };
            let (a, b) = solved.sketch.point(*a).zip(solved.sketch.point(*b)).ok_or("The axis endpoints are unavailable.")?;
            (AxisRef::SketchLine { curve: curve.clone() }, plane.to_world(a), plane.dir_to_world(b - a))
        }
        FreeCadAxis::Line { origin, dir } => (AxisRef::Line { origin: (*origin).into(), dir: (*dir).into() }, *origin, *dir),
    })
}

fn kind(
    s: &Session,
    sketch: &ImportedSketch,
    settings: &FreeCadOperationSettings,
    targets: &[String],
    has_base: bool,
) -> std::result::Result<FeatureKind, String> {
    let operation = |cut| {
        if cut {
            Operation::Cut
        } else if has_base {
            Operation::Join
        } else {
            Operation::NewBody
        }
    };
    let profiles = super::super::features::default_extrude_profiles(s, sketch.id);
    let mut feature = match settings {
        FreeCadOperationSettings::Extrude { length, length2, symmetric, reversed, cut, through_all, taper } => FeatureKind::Extrude {
            sketch: SketchId(sketch.id),
            profiles,
            extent: Extent {
                distance: format!("{length} mm").into(),
                convention: ExtrudeConvention::TotalDistance,
                direction: if *symmetric {
                    Direction::Symmetric
                } else if *cut != *reversed {
                    Direction::Negative
                } else {
                    Direction::Positive
                },
                distance2: length2.map(|v| format!("{v} mm").into()),
                start_offset: None,
                through_all: *through_all,
                taper: (taper.abs() > 1e-9).then(|| format!("{taper} deg").into()),
                to: None,
                to_offset: None,
                from: None,
            },
            operation: operation(*cut),
            targets: targets.iter().cloned().map(BodyName).collect(),
            thin: None,
        },
        FreeCadOperationSettings::Revolve { angle, angle2, symmetric, reversed, cut, axis: source_axis } => {
            let (axis, origin, direction) = axis(s, sketch, source_axis)?;
            let state = s.model.state();
            let solved = state.sketch(sketch.id).ok_or("The revolution's sketch could not be evaluated.")?;
            let unit = direction.normalized().ok_or("The revolution axis has no direction.")?;
            if unit.dot(solved.plane.normal()).abs() > 1e-7 || solved.plane.height(origin).abs() > 1e-6 {
                return Err("The revolution axis does not lie in the sketch plane.".into());
            }
            let local_origin = solved.plane.to_local(origin);
            let local_dir = Vec2::new(unit.dot(solved.plane.x), unit.dot(solved.plane.y));
            let mut signs = BTreeSet::new();
            for region in solidcraft_doc::select_profiles(solved, &profiles).map_err(|error| error.to_string())? {
                let canonical =
                    solidcraft_kernel::canonical_revolve_axis(&solved.plane, &region, local_origin, local_dir).map_err(|e| e.to_string())?;
                signs.insert(if canonical.dot(unit) < 0.0 { -1 } else { 1 });
            }
            if signs.len() > 1 {
                return Err("Profiles on both sides of the revolution axis need separate operations.".into());
            }
            let sign = f64::from(signs.first().copied().unwrap_or(1)) * if *reversed { -1.0 } else { 1.0 };
            FeatureKind::Revolve {
                sketch: SketchId(sketch.id),
                profiles,
                axis,
                angle: format!("{} deg", angle * sign).into(),
                angle2: angle2.map(|v| format!("{} deg", v * sign).into()),
                symmetric: *symmetric,
                convention: RevolveConvention::SketchNormal,
                operation: operation(*cut),
                targets: targets.iter().cloned().map(BodyName).collect(),
                to: None,
            }
        }
    };
    super::super::features::stable_profiles(s, &mut feature).map_err(|error| error.to_string())?;
    Ok(feature)
}

#[derive(Clone, Default)]
struct Outcome {
    bodies: Vec<String>,
    blocked: bool,
}

pub(super) fn import(s: &mut Session, operations: &[FreeCadOperation], sketches: &BTreeMap<String, ImportedSketch>) -> Result<Vec<Value>> {
    let mut outcomes = BTreeMap::<String, Outcome>::new();
    let by_name: BTreeMap<&str, &FreeCadOperation> = operations.iter().map(|op| (op.source_name.as_str(), op)).collect();
    let mut reports = Vec::new();
    let mut used_bodies: BTreeSet<String> = s.model.state().bodies.iter().map(|body| body.name.clone()).collect();
    for op in operations {
        let parent = op.base.as_ref().and_then(|base| outcomes.get(base)).cloned();
        let mut outcome = parent.clone().unwrap_or_default();
        let mut warnings = op.warnings.clone();
        let mut blockers = op.blockers.clone();
        if let Some(base) = op.base.as_deref().and_then(|name| by_name.get(name))
            && base.body.as_ref().map(|(name, _)| name) != op.body.as_ref().map(|(name, _)| name)
        {
            blockers.push(warning(
                "The preceding operation belongs to another Body.",
                "Cross-Body base-feature relationships require manual repair. Check the imported target and component before enabling this feature.",
            ));
        }
        if op.base.is_some() && parent.as_ref().is_none_or(|parent| parent.blocked) {
            blockers.push(warning(
                "An earlier operation needs repair.",
                format!(
                    "This operation depends on `{}`. Resolve its missing or suppressed predecessor before enabling this one. A [Not imported] placeholder records a missing operation that must be rebuilt manually.",
                    op.base.as_deref().unwrap_or_default()
                ),
            ));
        }
        let mut feature_id = None;
        if let (Some(settings), Some(sketch)) = (&op.settings, sketches.get(&op.profile)) {
            if op.body.as_ref().map(|(name, _)| name) != sketch.body.as_ref().map(|(name, _)| name) {
                blockers.push(warning(
                    "The operation uses a profile from another Body.",
                    "Cross-Body profile ownership requires manual repair. Check the feature's component and target body before enabling it.",
                ));
            }
            if sketch.needs_repair {
                blockers.push(warning("The profile sketch needs repair before this operation can be enabled.", "Its source attachment is unresolved or some profile geometry could not be imported. The saved plane and available geometry are retained. Redefine the sketch plane or repair the profile, then enable this operation and its dependents."));
            }
            let cut = matches!(settings, FreeCadOperationSettings::Extrude { cut: true, .. } | FreeCadOperationSettings::Revolve { cut: true, .. });
            let targets = if op.base.is_some() || cut {
                if outcome.bodies.is_empty() {
                    blockers.push(warning("The target body could not be identified.", "An explicit unresolved target is retained so enabling this feature cannot accidentally modify other imported bodies. Select the correct target body before enabling it."));
                    vec![format!("Unresolved FreeCAD body ({})", op.source_name)]
                } else {
                    outcome.bodies.clone()
                }
            } else {
                Vec::new()
            };
            match kind(s, sketch, settings, &targets, op.base.is_some()) {
                Ok(kind) => {
                    let label = if blockers.is_empty() { op.label.clone() } else { format!("{} [Needs repair]", op.label) };
                    let name = super::freecad_feature_name(s, &label);
                    let id = s.doc_mut().add_feature(kind, Some(&name))?;
                    let new_body = !cut && op.base.is_none();
                    let body_name = if new_body {
                        let base = sketch.body.as_ref().map(|(_, label)| label.as_str()).unwrap_or(&op.label);
                        let mut name = base.to_string();
                        let mut suffix = 2;
                        while used_bodies.contains(&name) {
                            name = format!("{base} ({suffix})");
                            suffix += 1;
                        }
                        used_bodies.insert(name.clone());
                        Some(name)
                    } else {
                        None
                    };
                    if let Some(feature) = s.doc_mut().feature_mut(id) {
                        feature.component = sketch.component;
                        feature.suppressed = !blockers.is_empty();
                        if let Some(name) = &body_name {
                            feature.body_names = vec![name.clone()];
                        }
                    }
                    s.doc_mut().name_feature_inputs(id);
                    let before: BTreeSet<String> = s.model.state().bodies.iter().map(|body| body.name.clone()).collect();
                    s.refresh();
                    if blockers.is_empty() {
                        if let Some(error) = s.model.result(id).and_then(|result| result.error.as_ref()).cloned() {
                            blockers.push(warning("SolidCraft could not rebuild this operation; it was imported suppressed.", error));
                            if let Some(feature) = s.doc_mut().feature_mut(id) {
                                feature.suppressed = true;
                                feature.name.push_str(" [Needs repair]");
                            }
                            s.refresh();
                        } else {
                            outcome.bodies = s
                                .model
                                .state()
                                .bodies
                                .iter()
                                .filter(|body| targets.contains(&body.name) || !before.contains(&body.name))
                                .map(|body| body.name.clone())
                                .collect();
                        }
                    }
                    if !blockers.is_empty()
                        && let Some(name) = body_name
                    {
                        outcome.bodies = vec![name];
                    }
                    feature_id = Some(id);
                }
                Err(error) => blockers.push(warning("This operation could not be represented as a native feature.", error)),
            }
        } else if op.settings.is_some() {
            blockers.push(warning(
                "The required profile sketch was not available.",
                format!(
                    "FreeCAD operation `{}` refers to profile `{}`. The profile could not be imported as an editable sketch.",
                    op.source_name, op.profile
                ),
            ));
        }
        let skipped = feature_id.is_none();
        outcome.blocked = !blockers.is_empty() || skipped;
        warnings.extend(blockers);
        if skipped {
            let mut detail = format!(
                "Source operation: {} ({})\n\nThis placeholder records a missing operation. It creates no geometry and cannot be enabled to recreate the original operation. Rebuild that operation manually before repairing its dependents.\n\n",
                op.label, op.source_name
            );
            for issue in &warnings {
                detail.push_str(&format!("{}\n{}\n\n", issue.summary, issue.detail));
            }
            let component = op
                .body
                .as_ref()
                .and_then(|(body, _)| sketches.values().find(|sketch| sketch.body.as_ref().is_some_and(|(source, _)| source == body)))
                .or_else(|| sketches.get(&op.profile))
                .map(|sketch| sketch.component)
                .unwrap_or(s.active_component);
            let name = super::freecad_feature_name(s, &format!("[Not imported] {}", op.label));
            let id = s.doc_mut().add_feature(
                FeatureKind::ImportPlaceholder { source: op.source_name.clone(), source_type: op.type_name.clone(), detail },
                Some(&name),
            )?;
            if let Some(feature) = s.doc_mut().feature_mut(id) {
                feature.component = component;
            }
            feature_id = Some(id);
            s.refresh();
        }
        reports.push(json!({"source": op.source_name, "name": op.label, "type": op.type_name, "feature": feature_id,
            "status": if skipped { "skipped" } else if outcome.blocked { "needs_repair" } else { "imported" }, "warnings": warnings}));
        outcomes.insert(op.source_name.clone(), outcome);
        used_bodies.extend(s.model.state().bodies.iter().map(|body| body.name.clone()));
    }
    if !operations.is_empty() {
        let mut saved = reports.iter().take(200).cloned().collect::<Vec<_>>();
        while serde_json::to_vec(&saved).map_or(usize::MAX, |bytes| bytes.len()) > 60_000 {
            if saved.pop().is_none() {
                break;
            }
        }
        let omitted = reports.len().saturating_sub(saved.len());
        s.doc_mut().metadata.set("org.solidcraft.freecad.operations", json!({"last_import": saved, "omitted": omitted}))?;
    }
    Ok(reports)
}
