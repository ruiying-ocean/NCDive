use std::path::{Path, PathBuf};

use netcdf::{AttributeValue, types::NcVariableType};

#[derive(Debug)]
pub struct Dataset {
    pub path: PathBuf,
    pub groups: Vec<GroupInfo>,
    pub variables: Vec<VariableInfo>,
}

#[derive(Debug)]
pub struct GroupInfo {
    pub path: String,
    pub dimensions: Vec<DimensionInfo>,
    pub attributes: Vec<AttributeInfo>,
}

#[derive(Debug)]
pub struct VariableInfo {
    pub path: String,
    pub data_type: String,
    pub dimensions: Vec<DimensionInfo>,
    pub attributes: Vec<AttributeInfo>,
}

#[derive(Debug)]
pub struct DimensionInfo {
    pub name: String,
    pub length: usize,
    pub unlimited: bool,
}

#[derive(Debug)]
pub struct AttributeInfo {
    pub name: String,
    pub value: Result<String, String>,
}

impl Dataset {
    pub fn load(path: &Path) -> Result<Self, String> {
        // Metadata is owned so the file can close before returning to the UI.
        let file = netcdf::open(path)
            .map_err(|error| format!("Could not open {}: {error}", path.display()))?;
        let mut dataset = Self {
            path: path.to_owned(),
            groups: vec![GroupInfo {
                path: "/".to_owned(),
                dimensions: file.dimensions().map(|d| read_dimension(&d)).collect(),
                attributes: read_attributes(file.attributes()),
            }],
            variables: read_variables("/", file.variables()),
        };
        if let Some(root) = file.root() {
            for group in root.groups() {
                dataset.read_group(&group, "/");
            }
        }
        Ok(dataset)
    }

    fn read_group(&mut self, group: &netcdf::Group<'_>, parent: &str) {
        let path = format!("{}/{}", parent.trim_end_matches('/'), group.name());
        self.variables
            .extend(read_variables(&path, group.variables()));
        self.groups.push(GroupInfo {
            path: path.clone(),
            dimensions: group.dimensions().map(|d| read_dimension(&d)).collect(),
            attributes: read_attributes(group.attributes()),
        });
        for child in group.groups() {
            self.read_group(&child, &path);
        }
    }
}

impl VariableInfo {
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|attribute| attribute.name == name)
            .and_then(|attribute| attribute.value.as_deref().ok())
    }

    pub fn shape(&self) -> String {
        if self.dimensions.is_empty() {
            "Scalar".to_owned()
        } else {
            self.dimensions
                .iter()
                .map(|dimension| dimension.length.to_string())
                .collect::<Vec<_>>()
                .join(" × ")
        }
    }
}

fn read_dimension(dimension: &netcdf::Dimension<'_>) -> DimensionInfo {
    DimensionInfo {
        name: dimension.name(),
        length: dimension.len(),
        unlimited: dimension.is_unlimited(),
    }
}

fn read_variables<'a>(
    group: &str,
    variables: impl Iterator<Item = netcdf::Variable<'a>>,
) -> Vec<VariableInfo> {
    variables
        .map(|variable| VariableInfo {
            path: format!("{}/{}", group.trim_end_matches('/'), variable.name()),
            data_type: match variable.vartype() {
                NcVariableType::Int(kind) => format!("{kind:?}").to_lowercase(),
                NcVariableType::Float(kind) => format!("{kind:?}").to_lowercase(),
                NcVariableType::Char => "character".to_owned(),
                NcVariableType::String => "string".to_owned(),
                NcVariableType::Compound(_) => "compound".to_owned(),
                NcVariableType::Opaque(_) => "opaque".to_owned(),
                NcVariableType::Enum(_) => "enum".to_owned(),
                NcVariableType::Vlen(_) => "variable length".to_owned(),
            },
            dimensions: variable.dimensions().iter().map(read_dimension).collect(),
            attributes: read_attributes(variable.attributes()),
        })
        .collect()
}

fn read_attributes<'a>(
    attributes: impl Iterator<Item = netcdf::Attribute<'a>>,
) -> Vec<AttributeInfo> {
    attributes
        .map(|attribute| AttributeInfo {
            name: attribute.name().to_owned(),
            value: attribute
                .value()
                .map(format_attribute)
                .map_err(|e| e.to_string()),
        })
        .collect()
}

fn format_attribute(value: AttributeValue) -> String {
    match value {
        AttributeValue::Str(value) => value,
        AttributeValue::Strs(value) => format!("{value:?}"),
        AttributeValue::Uchar(value) => value.to_string(),
        AttributeValue::Uchars(value) => format!("{value:?}"),
        AttributeValue::Schar(value) => value.to_string(),
        AttributeValue::Schars(value) => format!("{value:?}"),
        AttributeValue::Ushort(value) => value.to_string(),
        AttributeValue::Ushorts(value) => format!("{value:?}"),
        AttributeValue::Short(value) => value.to_string(),
        AttributeValue::Shorts(value) => format!("{value:?}"),
        AttributeValue::Uint(value) => value.to_string(),
        AttributeValue::Uints(value) => format!("{value:?}"),
        AttributeValue::Int(value) => value.to_string(),
        AttributeValue::Ints(value) => format!("{value:?}"),
        AttributeValue::Ulonglong(value) => value.to_string(),
        AttributeValue::Ulonglongs(value) => format!("{value:?}"),
        AttributeValue::Longlong(value) => value.to_string(),
        AttributeValue::Longlongs(value) => format!("{value:?}"),
        AttributeValue::Float(value) => value.to_string(),
        AttributeValue::Floats(value) => format!("{value:?}"),
        AttributeValue::Double(value) => value.to_string(),
        AttributeValue::Doubles(value) => format!("{value:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("samples")
            .join(name)
    }

    #[test]
    fn reads_classic_metadata_without_changing_the_file() {
        let path = sample("temperature.nc");
        let before = std::fs::read(&path).unwrap();
        let dataset = Dataset::load(&path).unwrap();
        assert_eq!(dataset.groups.len(), 1);
        assert_eq!(dataset.variables.len(), 4);
        let temperature = dataset
            .variables
            .iter()
            .find(|v| v.path == "/temperature")
            .unwrap();
        assert_eq!(temperature.data_type, "f32");
        assert_eq!(temperature.shape(), "2 × 3 × 4");
        assert_eq!(temperature.attribute("units"), Some("degC"));
        assert_eq!(temperature.attribute("_FillValue"), Some("-9999"));
        assert_eq!(temperature.dimensions[0].name, "time");
        assert!(temperature.dimensions[0].unlimited);
        assert_eq!(std::fs::read(path).unwrap(), before);
    }

    #[test]
    fn reads_nested_groups_and_distinguishes_duplicate_names() {
        let dataset = Dataset::load(&sample("groups.nc")).unwrap();
        assert_eq!(dataset.groups.len(), 3);
        let paths: Vec<_> = dataset.variables.iter().map(|v| v.path.as_str()).collect();
        assert_eq!(paths, ["/value", "/ocean/value", "/ocean/deep/value"]);
        assert_eq!(dataset.variables[0].shape(), "Scalar");
        assert_eq!(dataset.variables[1].attribute("units"), Some("m"));
        assert_eq!(
            dataset.groups[1].attributes[0].value.as_deref(),
            Ok("Ocean observations")
        );
    }

    #[test]
    fn reports_invalid_and_missing_files() {
        for name in ["temperature.cdl", "missing.nc"] {
            let error = Dataset::load(&sample(name)).unwrap_err();
            assert!(error.contains("Could not open"));
            assert!(error.contains(name));
        }
    }

    #[test]
    fn formats_numeric_and_string_attribute_arrays() {
        assert_eq!(
            format_attribute(AttributeValue::Shorts(vec![-5, 12])),
            "[-5, 12]"
        );
        assert_eq!(format_attribute(AttributeValue::Double(0.5)), "0.5");
        assert_eq!(
            format_attribute(AttributeValue::Strs(vec!["a".into(), "b".into()])),
            "[\"a\", \"b\"]"
        );
    }
}
