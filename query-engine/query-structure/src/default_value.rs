use prisma_value::{PrismaValue, PrismaValueType};
use std::fmt;

/// Represents a default specified on a field.
#[derive(Clone, PartialEq, Debug)]
pub struct DefaultValue {
    pub kind: DefaultKind,
    pub db_name: Option<String>,
}

/// Represents a default specified on a field.
#[derive(Clone, PartialEq)]
pub enum DefaultKind {
    /// a static value, e.g. `@default(1)`
    Single(PrismaValue),
    /// a dynamic value, e.g. `@default(uuid())`
    Expression(ValueGenerator),
}

impl DefaultKind {
    /// Does this match @default(autoincrement())?
    pub fn is_autoincrement(&self) -> bool {
        matches!(self, DefaultKind::Expression(generator) if generator.name == "autoincrement")
    }

    /// Does this match @default(ulid())?
    pub fn is_ulid(&self) -> bool {
        matches!(self, DefaultKind::Expression(generator) if generator.name == "ulid")
    }

    /// Does this match @default(cuid(_))?
    pub fn is_cuid(&self) -> bool {
        matches!(self, DefaultKind::Expression(generator) if generator.name == "cuid")
    }

    /// Does this match @default(dbgenerated(_))?
    pub fn is_dbgenerated(&self) -> bool {
        matches!(self, DefaultKind::Expression(generator) if generator.name == "dbgenerated")
    }

    /// Does this match @default(nanoid(_))?
    pub fn is_nanoid(&self) -> bool {
        matches!(self, DefaultKind::Expression(generator) if generator.name == "nanoid")
    }

    /// Does this match @default(now())?
    pub fn is_now(&self) -> bool {
        matches!(self, DefaultKind::Expression(generator) if generator.name == "now")
    }

    /// Does this match @default(uuid(_))?
    pub fn is_uuid(&self) -> bool {
        matches!(self, DefaultKind::Expression(generator) if generator.name == "uuid")
    }

    pub fn unwrap_single(self) -> PrismaValue {
        match self {
            DefaultKind::Single(val) => val,
            _ => panic!("called DefaultValue::unwrap_single() on wrong variant"),
        }
    }

    // Returns the dbgenerated function for a default value
    // intended for primary key values!
    pub fn to_dbgenerated_func(&self) -> Option<String> {
        match self {
            DefaultKind::Expression(ref expr) if expr.is_dbgenerated() => expr.args.first().map(|val| val.to_string()),
            _ => None,
        }
    }

    /// Returns either a copy of the contained single value or a non-evaluated generator call.
    pub fn get(&self) -> Option<PrismaValue> {
        match self {
            DefaultKind::Single(ref v) => Some(v.clone()),
            DefaultKind::Expression(g) if g.is_dbgenerated() || g.is_autoincrement() => None,
            DefaultKind::Expression(g) => Some(PrismaValue::GeneratorCall {
                name: g.name.clone().into(),
                args: g.args.clone(),
                return_type: g.return_type().unwrap_or(PrismaValueType::Any),
            }),
        }
    }

    /// Returns either a copy of the contained single value or produces a new
    /// value as defined by the expression.
    #[cfg(feature = "default_generators")]
    pub fn get_evaluated(&self) -> Option<PrismaValue> {
        match self {
            DefaultKind::Single(ref v) => Some(v.clone()),
            DefaultKind::Expression(g) => g.generate(),
        }
    }
}

impl DefaultValue {
    pub fn as_expression(&self) -> Option<&ValueGenerator> {
        match self.kind {
            DefaultKind::Expression(ref expr) => Some(expr),
            _ => None,
        }
    }

    pub fn as_single(&self) -> Option<&PrismaValue> {
        match self.kind {
            DefaultKind::Single(ref v) => Some(v),
            _ => None,
        }
    }

    /// Does this match @default(autoincrement())?
    pub fn is_autoincrement(&self) -> bool {
        self.kind.is_autoincrement()
    }

    /// Does this match @default(ulid())?
    pub fn is_ulid(&self) -> bool {
        self.kind.is_ulid()
    }

    /// Does this match @default(cuid(_))?
    pub fn is_cuid(&self) -> bool {
        self.kind.is_cuid()
    }

    /// Does this match @default(dbgenerated(_))?
    pub fn is_dbgenerated(&self) -> bool {
        self.kind.is_dbgenerated()
    }

    /// Does this match @default(nanoid(_))?
    pub fn is_nanoid(&self) -> bool {
        self.kind.is_nanoid()
    }

    /// Does this match @default(now())?
    pub fn is_now(&self) -> bool {
        self.kind.is_now()
    }

    /// Does this match @default(uuid(_))?
    pub fn is_uuid(&self) -> bool {
        self.kind.is_uuid()
    }

    pub fn new_expression(generator: ValueGenerator) -> Self {
        let kind = DefaultKind::Expression(generator);

        Self { kind, db_name: None }
    }

    pub fn new_single(value: PrismaValue) -> Self {
        let kind = DefaultKind::Single(value);

        Self { kind, db_name: None }
    }

    pub fn set_db_name(&mut self, name: impl ToString) {
        self.db_name = Some(name.to_string());
    }

    /// The default value constraint name.
    pub fn db_name(&self) -> Option<&str> {
        self.db_name.as_deref()
    }
}

#[derive(Clone)]
pub struct ValueGenerator {
    name: String,
    args: Vec<PrismaValue>,
    generator: ValueGeneratorFn,
}

impl ValueGenerator {
    pub fn new(name: String, args: Vec<PrismaValue>) -> Result<Self, String> {
        let generator = ValueGeneratorFn::new(name.as_ref(), args.as_ref())?;

        Ok(ValueGenerator { name, args, generator })
    }

    pub fn new_autoincrement() -> Self {
        ValueGenerator::new("autoincrement".to_owned(), vec![]).unwrap()
    }

    pub fn new_sequence(args: Vec<PrismaValue>) -> Self {
        ValueGenerator::new("sequence".to_owned(), args).unwrap()
    }

    pub fn new_dbgenerated(description: String) -> Self {
        let name = "dbgenerated".to_owned();

        if description.trim_matches('\0').is_empty() {
            ValueGenerator::new(name, Vec::new()).unwrap()
        } else {
            ValueGenerator::new(name, vec![PrismaValue::String(description)]).unwrap()
        }
    }

    pub fn new_auto() -> Self {
        ValueGenerator::new("auto".to_owned(), Vec::new()).unwrap()
    }

    pub fn new_now() -> Self {
        ValueGenerator::new("now".to_owned(), vec![]).unwrap()
    }

    pub fn new_ulid() -> Self {
        ValueGenerator::new("ulid".to_owned(), vec![]).unwrap()
    }

    pub fn new_cuid(version: u8) -> Self {
        ValueGenerator::new("cuid".to_owned(), vec![PrismaValue::Int(version as i64)]).unwrap()
    }

    pub fn new_uuid(version: u8) -> Self {
        ValueGenerator::new("uuid".to_owned(), vec![PrismaValue::Int(version as i64)]).unwrap()
    }

    pub fn new_nanoid(length: Option<u8>) -> Self {
        let name = "nanoid".to_owned();

        if let Some(length) = length {
            ValueGenerator::new(name, vec![PrismaValue::Int(length.into())]).unwrap()
        } else {
            ValueGenerator::new(name, vec![]).unwrap()
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn args(&self) -> &[PrismaValue] {
        &self.args
    }

    pub fn generator(&self) -> ValueGeneratorFn {
        self.generator
    }

    pub fn as_dbgenerated(&self) -> Option<&str> {
        if !(self.is_dbgenerated()) {
            return None;
        }

        self.args.first().and_then(|v| v.as_string())
    }

    #[cfg(feature = "default_generators")]
    pub fn generate(&self) -> Option<PrismaValue> {
        self.generator.invoke()
    }

    pub fn is_dbgenerated(&self) -> bool {
        self.name == "dbgenerated"
    }

    pub fn is_autoincrement(&self) -> bool {
        self.name == "autoincrement" || self.name == "sequence"
    }

    pub fn return_type(&self) -> Option<PrismaValueType> {
        self.generator.return_type()
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum ValueGeneratorFn {
    Uuid(u8),
    Cuid(u8),
    Ulid,
    Nanoid(Option<u8>),
    Now,
    Autoincrement,
    DbGenerated,
    Auto,
}

impl ValueGeneratorFn {
    fn new(name: &str, args: &[PrismaValue]) -> std::result::Result<Self, String> {
        match name {
            "ulid" => Ok(Self::Ulid),
            "cuid" => match args[..] {
                [PrismaValue::Int(version)] => Ok(Self::Cuid(version as u8)),
                _ => unreachable!(),
            },
            "uuid" => match args[..] {
                [PrismaValue::Int(version)] => Ok(Self::Uuid(version as u8)),
                _ => unreachable!(),
            },
            "nanoid" => match args[..] {
                [PrismaValue::Int(length)] => Ok(Self::Nanoid(Some(length as u8))),
                _ => Ok(Self::Nanoid(None)),
            },
            "now" => Ok(Self::Now),
            "autoincrement" => Ok(Self::Autoincrement),
            "sequence" => Ok(Self::Autoincrement),
            "dbgenerated" => Ok(Self::DbGenerated),
            "auto" => Ok(Self::Auto),
            _ => Err(format!("The function {name} is not a known function.")),
        }
    }

    #[cfg(feature = "default_generators")]
    fn invoke(&self) -> Option<PrismaValue> {
        match self {
            Self::Uuid(version) => Some(Self::generate_uuid(*version)),
            Self::Cuid(version) => Some(Self::generate_cuid(*version)),
            Self::Ulid => Some(Self::generate_ulid()),
            Self::Nanoid(length) => Some(Self::generate_nanoid(length)),
            Self::Now => Some(Self::generate_now()),
            Self::Autoincrement | Self::DbGenerated | Self::Auto => None,
        }
    }

    #[cfg(feature = "default_generators")]
    fn generate_ulid() -> PrismaValue {
        PrismaValue::String(ulid::Ulid::new().to_string())
    }

    #[cfg(feature = "default_generators")]
    fn generate_cuid(version: u8) -> PrismaValue {
        PrismaValue::String(match version {
            1 => cuid::cuid1(),
            2 => cuid::cuid2(),
            _ => panic!("Unknown `cuid` version: {version}"),
        })
    }

    #[cfg(feature = "default_generators")]
    fn generate_uuid(version: u8) -> PrismaValue {
        PrismaValue::Uuid(match version {
            4 => uuid::Uuid::new_v4(),
            7 => uuid::Uuid::now_v7(),
            _ => panic!("Unknown UUID version: {version}"),
        })
    }

    #[cfg(feature = "default_generators")]
    fn generate_nanoid(length: &Option<u8>) -> PrismaValue {
        if length.is_some() {
            let value: usize = usize::from(length.unwrap());
            PrismaValue::String(nanoid::nanoid!(value))
        } else {
            PrismaValue::String(nanoid::nanoid!())
        }
    }

    #[cfg(feature = "default_generators")]
    fn generate_now() -> PrismaValue {
        PrismaValue::DateTime(chrono::Utc::now().into())
    }

    pub fn return_type(&self) -> Option<PrismaValueType> {
        match self {
            ValueGeneratorFn::Uuid(_)
            | ValueGeneratorFn::Cuid(_)
            | ValueGeneratorFn::Ulid
            | ValueGeneratorFn::Nanoid(_) => Some(PrismaValueType::String),
            ValueGeneratorFn::Now => Some(PrismaValueType::Date),
            _ => None,
        }
    }
}

impl PartialEq for ValueGenerator {
    fn eq(&self, other: &Self) -> bool {
        self.name() == other.name() && self.args() == other.args()
    }
}

impl fmt::Debug for DefaultKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self {
            DefaultKind::Single(ref v) => write!(f, "DefaultValue::Single({v:?})"),
            DefaultKind::Expression(g) => write!(f, "DefaultValue::Expression({}(){:?})", g.name(), g.args),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DefaultValue, ValueGenerator};

    #[test]
    fn default_value_is_autoincrement() {
        let auto_increment_default = DefaultValue::new_expression(ValueGenerator::new_autoincrement());

        assert!(auto_increment_default.is_autoincrement());
    }

    #[test]
    fn default_value_is_now() {
        let now_default = DefaultValue::new_expression(ValueGenerator::new_now());

        assert!(now_default.is_now());
        assert!(!now_default.is_autoincrement());
    }

    #[test]
    fn default_value_is_uuidv4() {
        let uuid_default = DefaultValue::new_expression(ValueGenerator::new_uuid(4));

        assert!(uuid_default.is_uuid());
        assert!(!uuid_default.is_autoincrement());
    }

    #[test]
    fn default_value_is_uuidv7() {
        let uuid_default = DefaultValue::new_expression(ValueGenerator::new_uuid(7));

        assert!(uuid_default.is_uuid());
        assert!(!uuid_default.is_autoincrement());
    }

    #[test]
    fn default_value_is_cuidv1() {
        let cuid_default = DefaultValue::new_expression(ValueGenerator::new_cuid(1));

        assert!(cuid_default.is_cuid());
        assert!(!cuid_default.is_now());
    }

    #[test]
    fn default_value_is_cuidv2() {
        let cuid_default = DefaultValue::new_expression(ValueGenerator::new_cuid(2));

        assert!(cuid_default.is_cuid());
        assert!(!cuid_default.is_now());
    }

    #[test]
    fn default_value_is_ulid() {
        let ulid_default = DefaultValue::new_expression(ValueGenerator::new_ulid());

        assert!(ulid_default.is_ulid());
        assert!(!ulid_default.is_now());
    }

    #[test]
    fn default_value_is_nanoid() {
        let nanoid_default = DefaultValue::new_expression(ValueGenerator::new_nanoid(None));

        assert!(nanoid_default.is_nanoid());
        assert!(!nanoid_default.is_cuid());
    }

    #[test]
    fn default_value_is_dbgenerated() {
        let db_generated_default = DefaultValue::new_expression(ValueGenerator::new_dbgenerated("test".to_string()));

        assert!(db_generated_default.is_dbgenerated());
        assert!(!db_generated_default.is_now());
        assert!(!db_generated_default.is_autoincrement());
    }
}
