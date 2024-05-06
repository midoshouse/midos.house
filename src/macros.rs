macro_rules! as_variant {
    ($value:expr, $variant:path) => {
        if let $variant(field) = $value {
            Some(field)
        } else {
            None
        }
    };
    ($variant:path) => {
        |value| as_variant!(value, $variant)
    };
}

pub(crate) use as_variant;
