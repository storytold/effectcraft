use super::*;
use crate::load;

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

/// A binary FBX of the same node tree (`a:` children become typed arrays).
fn write_binary(nodes: &[FbxNode], version: u32, compress: bool) -> Vec<u8> {
    fn prop(out: &mut Vec<u8>, p: &Prop, compress: bool) {
        let array = |out: &mut Vec<u8>, t: u8, len: usize, raw: Vec<u8>| {
            out.push(t);
            out.extend((len as u32).to_le_bytes());
            let data = if compress { miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6) } else { raw };
            out.extend((compress as u32).to_le_bytes());
            out.extend((data.len() as u32).to_le_bytes());
            out.extend(data);
        };
        match p {
            Prop::Int(i) => match i32::try_from(*i) {
                Ok(v) => {
                    out.push(b'I');
                    out.extend(v.to_le_bytes());
                }
                Err(_) => {
                    out.push(b'L');
                    out.extend(i.to_le_bytes());
                }
            },
            Prop::Float(f) => {
                out.push(b'D');
                out.extend(f.to_le_bytes());
            }
            Prop::Str(s) => {
                let s = match s.split_once("::") {
                    Some((class, name)) => format!("{name}\0\x01{class}"),
                    None => s.clone(),
                };
                out.push(b'S');
                out.extend((s.len() as u32).to_le_bytes());
                out.extend(s.as_bytes());
            }
            Prop::Bytes(b) => {
                out.push(b'R');
                out.extend((b.len() as u32).to_le_bytes());
                out.extend(b);
            }
            Prop::Ints(v) => array(out, b'i', v.len(), v.iter().flat_map(|&i| (i as i32).to_le_bytes()).collect()),
            Prop::Floats(v) => array(out, b'd', v.len(), v.iter().flat_map(|f| f.to_le_bytes()).collect()),
        }
    }
    fn record(out: &mut Vec<u8>, n: &FbxNode, wide: bool, compress: bool) {
        let w = if wide { 8 } else { 4 };
        let start = out.len();
        out.extend(vec![0u8; 3 * w]);
        out.push(n.name.len() as u8);
        out.extend(n.name.as_bytes());
        let props = match n.child("a") {
            Some(a) if a.props.iter().all(|p| matches!(p, Prop::Int(_))) => vec![Prop::Ints(a.ints())],
            Some(a) => vec![Prop::Floats(a.floats())],
            None => n.props.clone(),
        };
        let pstart = out.len();
        for p in &props {
            prop(out, p, compress);
        }
        let plen = out.len() - pstart;
        let kids: Vec<&FbxNode> = n.children.iter().filter(|c| c.name != "a").collect();
        if !kids.is_empty() {
            for k in kids {
                record(out, k, wide, compress);
            }
            out.extend(vec![0u8; 3 * w + 1]);
        }
        let end = out.len();
        for (k, v) in [end, props.len(), plen].into_iter().enumerate() {
            let at = start + k * w;
            if wide {
                out[at..at + 8].copy_from_slice(&(v as u64).to_le_bytes());
            } else {
                out[at..at + 4].copy_from_slice(&(v as u32).to_le_bytes());
            }
        }
    }
    let wide = version >= 7500;
    let mut out = BINARY_MAGIC.to_vec();
    out.extend(version.to_le_bytes());
    for n in nodes {
        record(&mut out, n, wide, compress);
    }
    out.extend(vec![0u8; if wide { 25 } else { 13 }]);
    // Footer bytes after the closing null record.
    out.extend([0xfa; 16]);
    out
}

#[test]
fn ascii_fbx_scene() {
    let m = load("scene.fbx", ASCII.as_bytes(), &|_| None).unwrap();
    check_scene(&m);
}

#[test]
fn binary_fbx_matches_ascii() {
    let (_, doc) = read(ASCII.as_bytes()).unwrap();
    let ascii = parse(ASCII.as_bytes(), &|_| None).unwrap();
    for (version, compress) in [(7400, false), (7400, true), (7500, true)] {
        let bin = write_binary(&doc, version, compress);
        let (v, nodes) = read(&bin).unwrap();
        assert_eq!(v, version);
        assert_eq!(nodes.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), ["FBXHeaderExtension", "GlobalSettings", "Objects", "Connections"]);
        // Found by its magic even without the extension.
        let m = load("scene.bin", &bin, &|_| None).unwrap();
        assert_eq!(m, ascii, "version {version}, compressed {compress}");
    }
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

#[test]
fn hostile_fbx_files_fail_cleanly() {
    let (_, doc) = read(ASCII.as_bytes()).unwrap();
    let bin = write_binary(&doc, 7500, true);
    // Every truncation and every single-byte corruption returns (no panic, no runaway).
    for n in 0..bin.len() {
        let _ = parse(&bin[..n], &|_| None);
    }
    for i in 0..bin.len() {
        let mut b = bin.clone();
        b[i] ^= 0xff;
        let _ = parse(&b, &|_| None);
    }
    // An array claiming 4 G elements.
    let mut huge = BINARY_MAGIC.to_vec();
    huge.extend(7400u32.to_le_bytes());
    let body: Vec<u8> = b"d".iter().copied().chain(u32::MAX.to_le_bytes()).chain(0u32.to_le_bytes()).chain(8u32.to_le_bytes()).chain([0u8; 8]).collect();
    let start = huge.len();
    let end = start + 13 + 1 + body.len();
    huge.extend((end as u32).to_le_bytes());
    huge.extend(1u32.to_le_bytes());
    huge.extend((body.len() as u32).to_le_bytes());
    huge.push(1);
    huge.push(b'V');
    huge.extend(body);
    assert!(read(&huge).is_err());
    // Deep nesting, both encodings.
    assert!(read("A: {".repeat(100_000).as_bytes()).is_err());
    // Not an FBX scene at all, an old FBX version, and a scene without meshes.
    assert!(parse(b"\x00\x01garbage", &|_| None).is_err());
    let v6 = "FBXHeaderExtension:  {\n FBXVersion: 6100\n}\nObjects: {\n}\n";
    assert!(matches!(parse(v6.as_bytes(), &|_| None), Err(ModelError::Unsupported(_))));
    assert!(parse(b"Objects: {\n Model: 1, \"Model::Empty\", \"Null\" {\n }\n}\n", &|_| None).is_err());
}

/// A version-7400 binary FBX holding one top-level node `A` with uncompressed arrays of type
/// `t` (`b'b'`, `b'i'`, `b'f'`, `b'd'`, `b'l'`) and `len` elements each.
fn binary_arrays(arrays: &[(u8, usize)]) -> Vec<u8> {
    let mut props = vec![];
    for &(t, len) in arrays {
        let size = match t {
            b'd' | b'l' => 8,
            b'f' | b'i' => 4,
            _ => 1,
        };
        props.push(t);
        props.extend((len as u32).to_le_bytes());
        props.extend(0u32.to_le_bytes());
        props.extend(((len * size) as u32).to_le_bytes());
        props.extend(std::iter::repeat_n(1u8, len * size));
    }
    let mut out = BINARY_MAGIC.to_vec();
    out.extend(7400u32.to_le_bytes());
    let end = out.len() + 13 + 1 + props.len();
    out.extend((end as u32).to_le_bytes());
    out.extend((arrays.len() as u32).to_le_bytes());
    out.extend((props.len() as u32).to_le_bytes());
    out.push(1);
    out.push(b'A');
    out.extend(props);
    out
}

#[test]
fn fbx_arrays_are_charged_for_the_memory_they_keep() {
    // A boolean array is one byte per element on the wire but eight once kept as `Prop::Ints`:
    // the budget counts the eight (#1234).
    let n = 10_000;
    let bin = binary_arrays(&[(b'b', n)]);
    assert!(read_within(&bin, &Budget::new(n * 8)).is_ok());
    let e = read_within(&bin, &Budget::new(n * 8 - 1)).unwrap_err();
    assert!(e.to_string().contains("memory limit"), "{e}");
    // 32-bit floats widen to f64 the same way.
    assert!(read_within(&binary_arrays(&[(b'f', n)]), &Budget::new(n * 4)).is_err());
    // Several arrays each within the budget but not together.
    let bin = binary_arrays(&[(b'i', n), (b'd', n), (b'b', n)]);
    assert!(read_within(&bin, &Budget::new(n * 24)).is_ok());
    assert!(read_within(&bin, &Budget::new(n * 20)).is_err());
    // ASCII values count too.
    let (_, doc) = read(ASCII.as_bytes()).unwrap();
    let values = value_bytes(&doc).unwrap();
    assert!(values > 45 * std::mem::size_of::<Prop>(), "{values}");
    assert!(parse_within(ASCII.as_bytes(), &|_| None, values - 1).unwrap_err().to_string().contains("memory limit"));
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
    // A budget with room for one copy of the geometry's mesh, but not three, still imports.
    let one = parse(ASCII.as_bytes(), &|_| None).unwrap();
    let mesh = mesh_bytes(&one.meshes[0]).unwrap();
    let (_, doc) = read(instanced_scene().as_bytes()).unwrap();
    let values = value_bytes(&doc).unwrap();
    assert!(parse_within(instanced_scene().as_bytes(), &|_| None, values + 2 * mesh).is_ok());
    assert!(parse_within(instanced_scene().as_bytes(), &|_| None, values + mesh).is_err());
}
