use std::io::{Cursor, Write};

use super::*;

const DOCUMENT: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<Document SchemaVersion="4">
  <Properties Count="1"><Property name="Label" type="App::PropertyString"><String value="Fixture Project"/></Property></Properties>
  <Objects Count="4">
    <Object type="App::Part" name="Part" id="1"/>
    <Object type="PartDesign::Body" name="Body" id="2"/>
    <Object type="Sketcher::SketchObject" name="Profile" id="3"/>
    <Object type="Sketcher::SketchObject" name="Loose" id="4"/>
  </Objects>
  <ObjectData Count="4">
    <Object name="Part"><Properties Count="2">
      <Property name="Group" type="App::PropertyLinkList"><LinkList count="1"><Link value="Body"/></LinkList></Property>
      <Property name="Label" type="App::PropertyString"><String value="Bracket Part"/></Property>
    </Properties></Object>
    <Object name="Body"><Properties Count="2">
      <Property name="Group" type="App::PropertyLinkList"><LinkList count="1"><Link value="Profile"/></LinkList></Property>
      <Property name="Label" type="App::PropertyString"><String value="Main Body"/></Property>
    </Properties></Object>
    <Object name="Profile"><Properties Count="3">
      <Property name="Constraints" type="Sketcher::PropertyConstraintList"><ConstraintList count="7">
        <Constrain Type="1" Value="0" IsDriving="1" IsActive="1" IsInVirtualSpace="0" First="0" FirstPos="2" Second="1" SecondPos="1" Third="-2000"/>
        <Constrain Type="2" Value="0" IsDriving="1" IsActive="1" IsInVirtualSpace="0" First="0" FirstPos="0" Second="-2000" SecondPos="0" Third="-2000"/>
        <Constrain Type="6" Value="40" IsDriving="1" IsActive="1" IsInVirtualSpace="0" First="0" FirstPos="0" Second="-2000" SecondPos="0" Third="-2000"/>
        <Constrain Type="11" Value="3" IsDriving="1" IsActive="1" IsInVirtualSpace="0" First="2" FirstPos="0" Second="-2000" SecondPos="0" Third="-2000"/>
        <Constrain Type="12" Value="0" IsDriving="1" IsActive="1" IsInVirtualSpace="0" First="2" FirstPos="0" Second="3" SecondPos="0" Third="-2000"/>
        <Constrain Type="2" Value="0" IsDriving="1" IsActive="1" IsInVirtualSpace="0" First="2" FirstPos="3" Second="3" SecondPos="3" Third="-2000"/>
        <Constrain Type="999" Value="0" IsDriving="1" IsActive="1" IsInVirtualSpace="0" First="0" FirstPos="0" Second="-2000" SecondPos="0" Third="-2000"/>
      </ConstraintList></Property>
      <Property name="Geometry" type="Part::PropertyGeometryList"><GeometryList count="4">
        <Geometry type="Part::GeomLineSegment" id="1"><LineSegment StartX="0" StartY="0" EndX="40" EndY="0"/><Construction value="0"/></Geometry>
        <Geometry type="Part::GeomLineSegment" id="2"><LineSegment StartX="40" StartY="0" EndX="40" EndY="20"/><Construction value="1"/></Geometry>
        <Geometry type="Part::GeomCircle" id="3"><Circle CenterX="10" CenterY="10" Radius="3"/><Construction value="0"/></Geometry>
        <Geometry type="Part::GeomCircle" id="4"><Circle CenterX="30" CenterY="10" Radius="3"/><Construction value="0"/></Geometry>
      </GeometryList></Property>
      <Property name="Label" type="App::PropertyString"><String value="Mounting Profile"/></Property>
    </Properties></Object>
    <Object name="Loose"><Properties Count="2">
      <Property name="Geometry" type="Part::PropertyGeometryList"><GeometryList count="1">
        <Geometry type="Part::GeomArcOfCircle" id="1"><ArcOfCircle CenterX="5" CenterY="5" Radius="2" StartAngle="0" EndAngle="1.5707963267948966" AngleXU="0"/><Construction value="0"/></Geometry>
      </GeometryList></Property>
      <Property name="Label" type="App::PropertyString"><String value="Loose Sketch"/></Property>
    </Properties></Object>
  </ObjectData>
</Document>"#;

fn archive(xml: &[u8]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("Document.xml", zip::write::SimpleFileOptions::default()).unwrap();
    zip.write_all(xml).unwrap();
    zip.finish().unwrap().into_inner()
}

#[test]
fn circle_centers_on_sketch_axes_retain_geometric_constraints() {
    for (axis, dimension_type, x, y) in [(-2, 8, 0, 12), (-1, 7, 12, 0)] {
        let xml = format!(
            r#"<Document><Objects><Object name="BoltHoles" type="Sketcher::SketchObject"/></Objects>
        <ObjectData><Object name="BoltHoles"><Properties>
          <Property name="Geometry"><GeometryList><Geometry type="Part::GeomCircle">
            <Circle CenterX="{x}" CenterY="{y}" Radius="1.875"/>
          </Geometry></GeometryList></Property>
          <Property name="Constraints"><ConstraintList>
            <Constrain Type="13" First="0" FirstPos="3" Second="{axis}" SecondPos="0"/>
            <Constrain Type="{dimension_type}" Value="12" First="-1" FirstPos="1" Second="0" SecondPos="3"/>
            <Constrain Type="18" Value="3.75" First="0"/>
          </ConstraintList></Property>
        </Properties></Object></ObjectData></Document>"#
        );
        let project = read_freecad_sketches(&archive(xml.as_bytes()), "bolt-holes.FCStd").unwrap();
        let source = &project.sketches[0];
        assert!(source.warnings.is_empty(), "the built-in sketch axis is available: {:?}", source.warnings);
        let mut sketch = source.sketch.clone();
        let CurveKind::Circle { c: center, .. } = sketch.curves[0].kind else { panic!("expected a circle") };
        if axis == -2 {
            assert!(matches!(sketch.constraints[0].kind, ConstraintKind::VerticalPoints { p: 0, q } if q == center));
        } else {
            assert!(matches!(sketch.constraints[0].kind, ConstraintKind::HorizontalPoints { p: 0, q } if q == center));
        }
        assert!(!sketch.constraints[0].kind.is_dimension(), "point-on-axis must not become an editable zero dimension");
        assert_eq!(sketch.curves.len(), 1, "axis alignment needs no helper geometry");
        sketch.points[center].pos = Vec2::new(7.0, 18.0);
        let solved = solidcraft_sketch::solve(&mut sketch);
        assert!(solved.ok(), "{solved:?}");
        assert!((sketch.point(center).unwrap() - Vec2::new(f64::from(x), f64::from(y))).len() < 1e-7);
        assert!((sketch.radius(0).unwrap() - 1.875).abs() < 1e-7);

        if axis == -2 {
            let projected = xml.replace("Second=\"-2\"", "Second=\"-3\"");
            let project = read_freecad_sketches(&archive(projected.as_bytes()), "external.FCStd").unwrap();
            assert!(project.sketches[0].warnings.iter().any(|warning| warning.detail.contains("external geometry reference -3")));
        }
    }
}

#[test]
fn basic_operations_retain_profiles_dependencies_dimensions_and_axes() {
    let xml = include_str!("operations_fixture.xml");
    let project = read_freecad_sketches(&archive(xml.as_bytes()), "operations.FCStd").unwrap();
    assert_eq!(project.operations.len(), 4);
    let pocket = project.operations.iter().find(|op| op.source_name == "Pocket").unwrap();
    assert_eq!(pocket.base.as_deref(), Some("Pad"));
    assert_eq!(pocket.profile, "PocketProfile");
    assert!(pocket.blockers.is_empty());
    assert!(matches!(pocket.settings, Some(FreeCadOperationSettings::Extrude { length: 4.0, cut: true, .. })));
    let revolve = project.operations.iter().find(|op| op.source_name == "Revolution").unwrap();
    assert!(matches!(revolve.settings, Some(FreeCadOperationSettings::Revolve { angle: 270.0, axis: FreeCadAxis::SketchY, .. })));
    let profile = project.sketches.iter().find(|sketch| sketch.source_name == "PocketProfile").unwrap();
    assert!((profile.plane.origin.z - 10.0).abs() < 1e-9);

    let unsupported = xml.replace(
        "<Property name=\"Length\"><Float value=\"4\"/></Property>",
        "<Property name=\"Length\"><Float value=\"4\"/></Property><Property name=\"SideType\"><Integer value=\"99\"/></Property>",
    );
    let project = read_freecad_sketches(&archive(unsupported.as_bytes()), "unsupported.FCStd").unwrap();
    let pocket = project.operations.iter().find(|op| op.source_name == "Pocket").unwrap();
    assert!(!pocket.blockers.is_empty());
}

#[test]
fn hierarchy_geometry_and_constraints_are_imported() {
    let project = read_freecad_sketches(&archive(DOCUMENT.as_bytes()), "fixture.FCStd").unwrap();
    assert_eq!(project.label, "Fixture Project");
    assert_eq!(project.nodes.len(), 2, "the part and the ungrouped sketch are roots");
    let part = &project.nodes[0];
    assert_eq!((part.label.as_str(), part.kind), ("Bracket Part", FreeCadNodeKind::Part));
    assert_eq!(part.children[0].label, "Main Body");
    let profile_node = &part.children[0].children[0];
    assert_eq!((profile_node.label.as_str(), profile_node.kind), ("Mounting Profile", FreeCadNodeKind::Sketch));
    let info = profile_node.sketch.as_ref().unwrap();
    assert_eq!((info.geometry, info.constraints), (4, 6));
    let unknown = info.warnings.iter().find(|w| w.detail.contains("type 999")).unwrap();
    assert!(unknown.detail.contains("geometry 1 (line segment; FreeCAD index 0)"));

    let profile = project.sketches.iter().find(|s| s.source_name == "Profile").unwrap();
    assert_eq!(profile.path, ["Bracket Part", "Main Body", "Mounting Profile"]);
    assert_eq!(profile.parts, [("Part".into(), "Bracket Part".into())]);
    assert_eq!(profile.body, Some(("Body".into(), "Main Body".into())));
    assert_eq!(profile.sketch.curves.len(), 4);
    assert!(profile.sketch.curves[1].construction);
    assert!(matches!(profile.sketch.constraints[0].kind, ConstraintKind::Coincident { .. }));
    assert!(matches!(profile.sketch.constraints[2].kind, ConstraintKind::Length { value: 40.0, .. }));
    assert!(matches!(profile.sketch.constraints[3].kind, ConstraintKind::Radius { value: 3.0, .. }));
    assert!(matches!(profile.sketch.constraints[4].kind, ConstraintKind::Equal { .. }));
    assert!(matches!(profile.sketch.constraints[5].kind, ConstraintKind::HorizontalPoints { .. }));

    let loose = project.sketches.iter().find(|s| s.source_name == "Loose").unwrap();
    assert!(matches!(loose.sketch.curves[0].kind, CurveKind::Arc { .. }));
}

#[test]
fn malformed_and_missing_documents_are_errors() {
    assert!(read_freecad_sketches(b"not a zip", "bad.FCStd").is_err());
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("Other.xml", zip::write::SimpleFileOptions::default()).unwrap();
    zip.write_all(b"<x/>").unwrap();
    let no_document = zip.finish().unwrap().into_inner();
    assert!(read_freecad_sketches(&no_document, "missing.FCStd").is_err());
    assert!(read_freecad_sketches(&archive(b"<Document><broken>"), "broken.FCStd").is_err());
}

#[test]
fn unsupported_geometry_is_a_warning_not_a_failed_file() {
    let xml = DOCUMENT.replace(
        "Part::GeomArcOfCircle\" id=\"1\"><ArcOfCircle CenterX=\"5\" CenterY=\"5\" Radius=\"2\" StartAngle=\"0\" EndAngle=\"1.5707963267948966\" AngleXU=\"0\"/>",
        "Part::GeomBSplineCurve\" id=\"1\"><BSplineCurve/>",
    );
    let project = read_freecad_sketches(&archive(xml.as_bytes()), "unsupported.FCStd").unwrap();
    let loose = project.sketches.iter().find(|s| s.source_name == "Loose").unwrap();
    assert!(loose.sketch.curves.is_empty());
    let warning = loose.warnings.iter().find(|w| w.detail.contains("GeomBSplineCurve")).unwrap();
    assert!(warning.summary.contains("B-spline curve"));
    assert!(warning.detail.contains("line segments, circles, circular arcs and ellipses"));
}

#[test]
fn origin_plane_referenced_through_its_origin_is_not_a_face_attachment() {
    let xml = r#"<Document><Objects>
      <Object name="Body" type="PartDesign::Body"/><Object name="Origin" type="App::Origin"/>
      <Object name="XZ_Plane004" type="App::Plane"/><Object name="Sketch" type="Sketcher::SketchObject"/>
    </Objects><ObjectData>
      <Object name="Body"><Properties>
        <Property name="Group"><LinkList><Link value="Sketch"/></LinkList></Property>
        <Property name="Origin"><Link value="Origin"/></Property>
        <Property name="Placement"><PropertyPlacement Px="100" Py="0" Pz="5" Q0="0" Q1="0" Q2="0" Q3="1"/></Property>
      </Properties></Object>
      <Object name="Origin"><Properties>
        <Property name="OriginFeatures"><LinkList><Link value="XZ_Plane004"/></LinkList></Property>
      </Properties></Object>
      <Object name="XZ_Plane004"><Properties>
        <Property name="Label"><String value="XZ plane"/></Property>
      </Properties></Object>
      <Object name="Sketch"><Properties>
        <Property name="MapMode"><Integer value="5"/></Property>
        <Property name="AttachmentSupport"><LinkSubList><Link obj="Origin" sub="XZ_Plane004"/></LinkSubList></Property>
        <Property name="Placement"><PropertyPlacement Px="0" Py="0" Pz="0" Q0="0.7071067811865475" Q1="0" Q2="0" Q3="0.7071067811865476"/></Property>
      </Properties></Object>
    </ObjectData></Document>"#;
    let project = read_freecad_sketches(&archive(xml.as_bytes()), "origin-plane.FCStd").unwrap();
    let sketch = &project.sketches[0];
    assert!(!sketch.attachment_unresolved);
    assert!(sketch.warnings.is_empty());
    assert_eq!(sketch.plane_description, "Origin plane: XZ plane");
    assert!((sketch.plane.origin - Vec3::new(100.0, 0.0, 5.0)).len() < 1e-9);
    assert!((sketch.plane.y - Vec3::Z).len() < 1e-9);
    let unowned = xml.replace("<Link value=\"XZ_Plane004\"/>", "<Link value=\"UnrelatedPlane\"/>");
    let project = read_freecad_sketches(&archive(unowned.as_bytes()), "unowned-plane.FCStd").unwrap();
    assert!(!project.sketches[0].attachment_unresolved, "an explicitly named App::Plane retains the saved static plane");
    assert!(project.sketches[0].warnings.iter().any(|warning| warning.summary.contains("Origin link is stale")));
    let missing = xml.replace("sub=\"XZ_Plane004\"", "sub=\"MissingPlane\"");
    let project = read_freecad_sketches(&archive(missing.as_bytes()), "missing-plane.FCStd").unwrap();
    assert!(project.sketches[0].attachment_unresolved, "a missing plane cannot be inferred from its name");
}

#[test]
fn planes_keep_saved_frames_parent_placements_and_report_face_fallbacks() {
    let xml = r#"<Document><Objects>
      <Object name="Part" type="App::Part"/><Object name="Body" type="PartDesign::Body"/>
      <Object name="Datum" type="PartDesign::Plane"/><Object name="XZ" type="App::Plane"/>
      <Object name="Sketch" type="Sketcher::SketchObject"/><Object name="FaceSketch" type="Sketcher::SketchObject"/>
      <Object name="Solid" type="PartDesign::Pad"/>
    </Objects><ObjectData>
      <Object name="Part"><Properties>
        <Property name="Group"><LinkList><Link value="Body"/></LinkList></Property>
        <Property name="Placement"><PropertyPlacement Px="100" Py="0" Pz="0" Q0="0" Q1="0" Q2="0" Q3="1"/></Property>
      </Properties></Object>
      <Object name="Body"><Properties>
        <Property name="Group"><LinkList><Link value="Datum"/><Link value="Sketch"/><Link value="FaceSketch"/></LinkList></Property>
        <Property name="Placement"><PropertyPlacement Px="0" Py="0" Pz="5" Q0="0" Q1="0" Q2="0" Q3="1"/></Property>
      </Properties></Object>
      <Object name="Datum"><Properties>
        <Property name="Label"><String value="Mounting plane"/></Property>
        <Property name="Placement"><PropertyPlacement Px="0" Py="0" Pz="12" Q0="0.7071067811865475" Q1="0" Q2="0" Q3="0.7071067811865476"/></Property>
      </Properties></Object>
      <Object name="Sketch"><Properties>
        <Property name="MapMode"><Integer value="5"/></Property>
        <Property name="AttachmentSupport"><LinkSubList><Link obj="Datum" sub=""/></LinkSubList></Property>
        <Property name="Placement"><PropertyPlacement Px="0" Py="-7" Pz="12" Q0="0.7071067811865475" Q1="0" Q2="0" Q3="0.7071067811865476"/></Property>
      </Properties></Object>
      <Object name="FaceSketch"><Properties>
        <Property name="MapMode"><Integer value="5"/></Property>
        <Property name="Support"><LinkSub value="Solid"><Sub value="Face2"/></LinkSub></Property>
        <Property name="Placement"><PropertyPlacement Px="0" Py="0" Pz="50" Q0="0" Q1="0" Q2="0" Q3="1"/></Property>
      </Properties></Object>
      <Object name="Solid"><Properties/></Object>
    </ObjectData></Document>"#;
    let project = read_freecad_sketches(&archive(xml.as_bytes()), "planes.FCStd").unwrap();
    let sketch = project.sketches.iter().find(|s| s.source_name == "Sketch").unwrap();
    assert!((sketch.plane.origin - Vec3::new(100.0, -7.0, 17.0)).len() < 1e-9);
    assert!((sketch.plane.x - Vec3::X).len() < 1e-9);
    assert!((sketch.plane.y - Vec3::Z).len() < 1e-9, "FreeCAD XZ uses +Z, unlike SolidCraft's named XZ");
    assert!(sketch.warnings.is_empty());
    let (_, label, datum) = sketch.datum.as_ref().unwrap();
    assert_eq!(label, "Mounting plane");
    assert!((datum.origin - Vec3::new(100.0, 0.0, 17.0)).len() < 1e-9);
    let face = project.sketches.iter().find(|s| s.source_name == "FaceSketch").unwrap();
    assert_eq!(face.plane, Plane::XY);
    assert!(face.warnings.iter().any(|w| w.detail.contains("Face2")));
    assert!(face.datum.is_none());

    let invalid = xml.replace("Q0=\"0.7071067811865475\" Q1=\"0\" Q2=\"0\" Q3=\"0.7071067811865476\"", "Q0=\"0\" Q1=\"0\" Q2=\"0\" Q3=\"0\"");
    let project = read_freecad_sketches(&archive(invalid.as_bytes()), "invalid-plane.FCStd").unwrap();
    let sketch = project.sketches.iter().find(|s| s.source_name == "Sketch").unwrap();
    assert_eq!(sketch.plane, Plane::XY);
    assert!(sketch.warnings.iter().any(|w| w.detail.contains("invalid placement")));
}
