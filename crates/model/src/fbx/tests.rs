use super::*;
use crate::{AlphaMode, load, vec3};

/// A Z-up scene: a Null parent with a mesh child (rotated, scaled, with a geometric offset)
/// holding a quad and a triangle with a material each.
pub(crate) const ASCII: &str = r#"; FBX 7.4.0 project file
; ----------------------------------------------------
FBXHeaderExtension:  {
	FBXHeaderVersion: 1003
	FBXVersion: 7400
	Creator: "EffectCraft test"
}
GlobalSettings:  {
	Version: 1000
	Properties70:  {
		P: "UpAxis", "int", "Integer", "",2
		P: "UpAxisSign", "int", "Integer", "",1
		P: "FrontAxis", "int", "Integer", "",1
		P: "FrontAxisSign", "int", "Integer", "",-1
		P: "CoordAxis", "int", "Integer", "",0
		P: "CoordAxisSign", "int", "Integer", "",1
	}
}
Objects:  {
	Geometry: 100, "Geometry::Shape", "Mesh" {
		Vertices: *15 {
			a: 0,0,0,1,0,0,1,1,0,0,1,0,0.5,0.5,1
		}
		PolygonVertexIndex: *7 {
			a: 0,1,2,-4,0,1,-5
		}
		LayerElementNormal: 0 {
			MappingInformationType: "ByPolygon"
			ReferenceInformationType: "Direct"
			Normals: *6 {
				a: 0,0,1,0,-1,0
			}
		}
		LayerElementUV: 0 {
			MappingInformationType: "ByPolygonVertex"
			ReferenceInformationType: "IndexToDirect"
			UV: *8 {
				a: 0,0,1,0,1,1,0,1
			}
			UVIndex: *7 {
				a: 0,1,2,3,0,1,2
			}
		}
		LayerElementMaterial: 0 {
			MappingInformationType: "ByPolygon"
			ReferenceInformationType: "IndexToDirect"
			Materials: *2 {
				a: 0,1
			}
		}
	}
	Model: 200, "Model::Parent", "Null" {
		Properties70:  {
			P: "Lcl Translation", "Lcl Translation", "", "A",10,0,0
		}
	}
	Model: 300, "Model::Shape", "Mesh" {
		Properties70:  {
			P: "Lcl Rotation", "Lcl Rotation", "", "A",0,0,90
			P: "Lcl Scaling", "Lcl Scaling", "", "A",2,2,2
			P: "GeometricTranslation", "Vector3D", "Vector", "",0,0,1
		}
		Shading: T
	}
	Material: 400, "Material::Red", "" {
		ShadingModel: "phong"
		Properties70:  {
			P: "DiffuseColor", "Color", "", "A",1,0,0
			P: "ShininessExponent", "Number", "", "A",98
		}
	}
	Material: 500, "Material::Glass", "" {
		ShadingModel: "lambert"
		Properties70:  {
			P: "DiffuseColor", "Color", "", "A",0,0,1
			P: "Opacity", "double", "Number", "",0.25
		}
	}
}
Connections:  {
	;Model::Parent, Model::RootNode
	C: "OO",200,0
	C: "OO",300,200
	C: "OO",100,300
	C: "OO",400,300
	C: "OO",500,300
}
"#;

fn check_scene(m: &Model) {
    assert_eq!(m.meshes.len(), 1);
    let names: Vec<&str> = m.nodes.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(names, ["FBX axes", "Parent", "Shape", "Shape geometry"]);
    assert_eq!(m.roots, vec![0]);
    assert_eq!(m.nodes[0].children, vec![1]);
    assert_eq!(m.nodes[1].children, vec![2]);
    assert_eq!(m.nodes[2].children, vec![3]);
    assert_eq!(m.nodes[3].mesh, Some(0));

    let prims = &m.meshes[0].primitives;
    assert_eq!(prims.len(), 2);
    let (quad, tri) = (&prims[0], &prims[1]);
    assert_eq!((quad.positions.len(), quad.indices.clone()), (4, vec![0, 1, 2, 0, 2, 3]));
    assert_eq!((tri.positions.len(), tri.triangle_count()), (3, 1));
    assert_eq!(quad.normals, vec![[0.0, 0.0, 1.0]; 4]);
    assert_eq!(tri.normals, vec![[0.0, -1.0, 0.0]; 3]);
    // UVs through the index, v flipped to image rows.
    assert_eq!(quad.uvs, vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]);

    assert_eq!(m.materials.len(), 2);
    let (red, glass) = (&m.materials[quad.material.unwrap()], &m.materials[tri.material.unwrap()]);
    assert_eq!(red.name, "Red");
    assert_eq!(red.base_color, [1.0, 0.0, 0.0, 1.0]);
    assert!((red.roughness - 0.02f32.sqrt()).abs() < 1e-6 && red.metallic == 0.0);
    assert_eq!(glass.name, "Glass");
    assert_eq!(glass.base_color, [0.0, 0.0, 1.0, 0.25]);
    assert_eq!((glass.alpha_mode, glass.roughness), (AlphaMode::Blend, 1.0));

    // Z-up → Y-up, parent translation, Z rotation, scale and the geometric offset.
    let inst = m.instances(None, 0.0);
    assert_eq!(inst.len(), 1);
    let apex = inst[0].world.apply(vec3(0.5, 0.5, 1.0));
    assert!((apex.x - 9.0).abs() < 1e-9 && (apex.y - 4.0).abs() < 1e-9 && (apex.z + 1.0).abs() < 1e-9, "{apex:?}");
    let (lo, hi) = m.bounds().unwrap();
    let close = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9);
    assert!(close(lo, [8.0, 2.0, -2.0]) && close(hi, [10.0, 4.0, 0.0]), "{lo:?} {hi:?}");
}

#[test]
fn ascii_fbx_scene() {
    let m = load("scene.fbx", ASCII.as_bytes(), &|_| None).unwrap();
    check_scene(&m);
}

#[test]
fn embedded_and_relative_textures() {
    let png = {
        let img = image::RgbaImage::from_pixel(2, 1, image::Rgba([0, 255, 0, 255]));
        let mut b = std::io::Cursor::new(vec![]);
        img.write_to(&mut b, image::ImageFormat::Png).unwrap();
        b.into_inner()
    };
    let objects = r#"
	Texture: 600, "Texture::Base", "" {
		RelativeFilename: "maps\base.png"
	}
	Texture: 700, "Texture::Glow", "" {
		RelativeFilename: "missing.png"
	}
	Video: 800, "Video::Glow", "Clip" {
		Content: "BASE64"
	}
}
Connections:  {"#
        .replace("BASE64", &b64(&png));
    let doc = ASCII.replace("}\nConnections:  {", &objects).replace(
        "C: \"OO\",500,300\n",
        "C: \"OO\",500,300\n\tC: \"OP\",600,400, \"DiffuseColor\"\n\tC: \"OP\",700,500, \"EmissiveColor\"\n\tC: \"OO\",800,700\n",
    );
    let m = load("scene.fbx", doc.as_bytes(), &|uri| (uri == "maps/base.png").then(|| png.clone())).unwrap();
    assert_eq!(m.textures.len(), 2);
    let red = m.materials.iter().find(|x| x.name == "Red").unwrap();
    assert_eq!(m.textures[red.base_color_tex.unwrap().texture].data[0], [0.0, 1.0, 0.0, 1.0]);
    let glass = m.materials.iter().find(|x| x.name == "Glass").unwrap();
    assert!(glass.emissive_tex.is_some());
    assert_eq!(glass.emissive, [1.0; 3]);
}

fn b64(b: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            if k <= c.len() {
                s.push(A[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// [`ASCII`] with two more Models instancing its geometry: `Copy` with the same two materials,
/// `Red only` with just the first.
fn instanced_scene() -> String {
    ASCII
        .replace(
            "\tMaterial: 400,",
            "\tModel: 301, \"Model::Copy\", \"Mesh\" {\n\t}\n\tModel: 302, \"Model::Red only\", \"Mesh\" {\n\t}\n\tMaterial: 400,",
        )
        .replace(
            "\tC: \"OO\",500,300\n",
            "\tC: \"OO\",500,300\n\tC: \"OO\",301,0\n\tC: \"OO\",100,301\n\tC: \"OO\",400,301\n\tC: \"OO\",500,301\n\tC: \"OO\",302,0\n\tC: \"OO\",100,302\n\tC: \"OO\",400,302\n",
        )
}

#[test]
fn fbx_models_instancing_one_geometry_share_its_mesh() {
    let m = parse(instanced_scene().as_bytes(), &|_| None).unwrap();
    let mesh_of = |name: &str| {
        let n = m.nodes.iter().position(|n| n.name == name).unwrap();
        // The mesh sits on the node, or on its geometric-offset child.
        m.nodes[n].mesh.or_else(|| m.nodes[n].children.iter().find_map(|&c| m.nodes[c].mesh)).unwrap()
    };
    assert_eq!(mesh_of("Shape"), mesh_of("Copy"), "same geometry and materials: one mesh");
    assert_ne!(mesh_of("Shape"), mesh_of("Red only"), "different materials: its own mesh");
    assert_eq!(m.meshes.len(), 2);
}

#[test]
fn texture_aliases_of_one_image_share_its_pixels() {
    let png = {
        let img = image::RgbaImage::from_pixel(2, 1, image::Rgba([0, 255, 0, 255]));
        let mut b = std::io::Cursor::new(vec![]);
        img.write_to(&mut b, image::ImageFormat::Png).unwrap();
        b.into_inner()
    };
    let objects = r#"
	Texture: 600, "Texture::Base", "" {
		RelativeFilename: "maps\base.png"
	}
	Texture: 700, "Texture::Base again", "" {
		RelativeFilename: "maps\base.png"
	}
}
Connections:  {"#;
    let doc = ASCII
        .replace("}\nConnections:  {", objects)
        .replace("C: \"OO\",500,300\n", "C: \"OO\",500,300\n\tC: \"OP\",600,400, \"DiffuseColor\"\n\tC: \"OP\",700,500, \"DiffuseColor\"\n");
    let resolve = |uri: &str| (uri == "maps/base.png").then(|| png.clone());
    let m = parse(doc.as_bytes(), &resolve).unwrap();
    assert_eq!(m.textures.len(), 1);
    assert!(m.materials.iter().all(|x| x.base_color_tex.map(|t| t.texture) == Some(0)));
    assert!(m.warnings.is_empty());
}

/// A PNG that is only a header claiming `w`×`h` pixels (an empty data chunk follows).
fn png_header_only(w: u32, h: u32) -> Vec<u8> {
    fn chunk(png: &mut Vec<u8>, ty: &[u8], data: &[u8]) {
        let body = [ty, data].concat();
        let mut crc = 0xFFFF_FFFFu32;
        for &b in &body {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        png.extend((data.len() as u32).to_be_bytes());
        png.extend(&body);
        png.extend((!crc).to_be_bytes());
    }
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    chunk(&mut png, b"IHDR", &[w.to_be_bytes(), h.to_be_bytes(), [8, 6, 0, 0]].concat().iter().copied().chain([0]).collect::<Vec<u8>>());
    chunk(&mut png, b"IDAT", &[]);
    png
}

#[test]
fn a_texture_past_the_limits_is_left_out_with_a_warning_not_a_failed_import() {
    let objects = r#"
	Texture: 600, "Texture::Base", "" {
		RelativeFilename: "maps\base.png"
	}
}
Connections:  {"#;
    let doc = ASCII.replace("}\nConnections:  {", objects).replace("C: \"OO\",500,300\n", "C: \"OO\",500,300\n\tC: \"OP\",600,400, \"DiffuseColor\"\n");
    // 30000×30000 pixels would take ~14 GiB; the header alone is refused, before anything is allocated.
    let huge = png_header_only(30000, 30000);
    let resolve = |_: &str| Some(huge.clone());
    let m = parse(doc.as_bytes(), &resolve).unwrap();
    assert!(m.textures.is_empty() && m.materials.iter().all(|x| x.base_color_tex.is_none()));
    assert!(m.warnings.iter().any(|w| w.contains("30000")), "{:?}", m.warnings);
}

#[test]
fn texture_files_resolving_to_one_image_share_it() {
    let png = {
        let img = image::RgbaImage::from_pixel(2, 1, image::Rgba([0, 255, 0, 255]));
        let mut b = std::io::Cursor::new(vec![]);
        img.write_to(&mut b, image::ImageFormat::Png).unwrap();
        b.into_inner()
    };
    let objects = r#"
	Texture: 600, "Texture::Base", "" {
		RelativeFilename: "maps\base.png"
	}
	Texture: 700, "Texture::Other spelling", "" {
		RelativeFilename: "missing\base.png"
		FileName: "maps\base.png"
	}
}
Connections:  {"#;
    let doc = ASCII
        .replace("}\nConnections:  {", objects)
        .replace("C: \"OO\",500,300\n", "C: \"OO\",500,300\n\tC: \"OP\",600,400, \"DiffuseColor\"\n\tC: \"OP\",700,500, \"DiffuseColor\"\n");
    let resolve = |uri: &str| (uri == "maps/base.png").then(|| png.clone());
    assert_eq!(parse(doc.as_bytes(), &resolve).unwrap().textures.len(), 1);
}
