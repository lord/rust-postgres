use regex::Regex;
use std::ascii::AsciiExt;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Write, BufWriter};
use std::path::Path;
use marksman_escape::Escape;

use snake_to_camel;

const PG_TYPE_H: &'static str = include_str!("pg_type.h");
const PG_RANGE_H: &'static str = include_str!("pg_range.h");

struct Type {
    name: &'static str,
    variant: String,
    kind: &'static str,
    element: u32,
    doc: String,
}

pub fn build(path: &Path) {
    let mut file = BufWriter::new(File::create(path.join("types/type_gen.rs")).unwrap());

    let ranges = parse_ranges();
    let types = parse_types(&ranges);

    make_header(&mut file);
    make_enum(&mut file, &types);
    make_display_impl(&mut file);
    make_impl(&mut file, &types);
}

fn parse_ranges() -> BTreeMap<u32, u32> {
    let mut ranges = BTreeMap::new();

    for line in PG_RANGE_H.lines() {
        if !line.starts_with("DATA") {
            continue;
        }

        let split = line.split_whitespace().collect::<Vec<_>>();

        let oid = split[2].parse().unwrap();
        let element = split[3].parse().unwrap();

        ranges.insert(oid, element);
    }

    ranges
}

fn parse_types(ranges: &BTreeMap<u32, u32>) -> BTreeMap<u32, Type> {
    let doc_re = Regex::new(r#"DESCR\("([^"]+)"\)"#).unwrap();
    let range_vector_re = Regex::new("(range|vector)$").unwrap();
    let array_re = Regex::new("^_(.*)").unwrap();

    let mut types = BTreeMap::new();

    let mut lines = PG_TYPE_H.lines().peekable();
    while let Some(line) = lines.next() {
        if !line.starts_with("DATA") {
            continue;
        }

        let split = line.split_whitespace().collect::<Vec<_>>();

        let oid = split[3].parse().unwrap();

        let name = split[5];

        let variant = match name {
            "anyarray" => "AnyArray".to_owned(),
            name => {
                let variant = range_vector_re.replace(name, "_$1");
                let variant = array_re.replace(&variant, "$1_array");
                snake_to_camel(&variant)
            }
        };

        let kind = split[11];

        // we need to be able to pull composite fields and enum variants at runtime
        if kind == "C" || kind == "E" {
            continue;
        }

        let element = if let Some(&element) = ranges.get(&oid) {
            element
        } else {
            split[16].parse().unwrap()
        };

        let doc = array_re.replace(name, "$1[]");
        let mut doc = doc.to_ascii_uppercase();

        let descr = lines.peek()
                         .and_then(|line| doc_re.captures(line))
                         .and_then(|captures| captures.at(1));
        if let Some(descr) = descr {
            doc.push_str(" - ");
            doc.push_str(descr);
        }
        let doc = Escape::new(doc.as_bytes().iter().cloned()).collect();
        let doc = String::from_utf8(doc).unwrap();

        let type_ = Type {
            name: name,
            variant: variant,
            kind: kind,
            element: element,
            doc: doc,
        };

        types.insert(oid, type_);
    }

    types
}

fn make_header(w: &mut BufWriter<File>) {
    write!(w,
"// Autogenerated file - DO NOT EDIT
use std::fmt;

use types::{{Oid, Kind, Other}};

"
           ).unwrap();
}

fn make_enum(w: &mut BufWriter<File>, types: &BTreeMap<u32, Type>) {
    write!(w,
"/// A Postgres type.
#[derive(PartialEq, Eq, Clone, Debug)]
pub enum Type {{
"
           ).unwrap();

    for type_ in types.values() {
        write!(w,
"    /// {}
    {},
"
               , type_.doc, type_.variant).unwrap();
    }

    write!(w,
r"    /// An unknown type.
    Other(Other),
}}

"         ).unwrap();
}

fn make_display_impl(w: &mut BufWriter<File>) {
    write!(w,
r#"impl fmt::Display for Type {{
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {{
        match self.schema() {{
            "public" | "pg_catalog" => {{}}
            schema => write!(fmt, "{{}}.", schema)?,
        }}
        fmt.write_str(self.name())
    }}
}}

"#,
       ).unwrap();
}

fn make_impl(w: &mut BufWriter<File>, types: &BTreeMap<u32, Type>) {
    write!(w,
"impl Type {{
    /// Returns the `Type` corresponding to the provided `Oid` if it
    /// corresponds to a built-in type.
    pub fn from_oid(oid: Oid) -> Option<Type> {{
        match oid {{
",
           ).unwrap();

    for (oid, type_) in types {
        write!(w,
"            {} => Some(Type::{}),
",
               oid, type_.variant).unwrap();
    }

    write!(w,
"            _ => None,
        }}
    }}

    /// Returns the OID of the `Type`.
    pub fn oid(&self) -> Oid {{
        match *self {{
",
           ).unwrap();


    for (oid, type_) in types {
        write!(w,
"            Type::{} => {},
",
               type_.variant, oid).unwrap();
    }

    write!(w,
"            Type::Other(ref u) => u.oid(),
        }}
    }}

    /// Returns the kind of this type.
    pub fn kind(&self) -> &Kind {{
        match *self {{
",
           ).unwrap();

    for type_ in types.values() {
        let kind = match type_.kind {
            "P" => "Pseudo".to_owned(),
            "A" => format!("Array(Type::{})", types[&type_.element].variant),
            "R" => format!("Range(Type::{})", types[&type_.element].variant),
            _ => "Simple".to_owned(),
        };

        write!(w,
"            Type::{} => {{
                const V: &'static Kind = &Kind::{};
                V
            }}
",
               type_.variant, kind).unwrap();
    }

    write!(w,
r#"            Type::Other(ref u) => u.kind(),
        }}
    }}

    /// Returns the schema of this type.
    pub fn schema(&self) -> &str {{
        match *self {{
            Type::Other(ref u) => u.schema(),
            _ => "pg_catalog",
        }}
    }}

    /// Returns the name of this type.
    pub fn name(&self) -> &str {{
        match *self {{
"#,
          ).unwrap();

    for type_ in types.values() {
        write!(w,
r#"            Type::{} => "{}",
"#,
               type_.variant, type_.name).unwrap();
    }

    write!(w,
"            Type::Other(ref u) => u.name(),
        }}
    }}
}}
"
           ).unwrap();
}
