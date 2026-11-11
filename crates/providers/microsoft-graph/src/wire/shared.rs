use serde::Deserialize;

#[derive(Default)]
pub(super) enum WireField<T> {
    #[default]
    Missing,
    Null,
    Value(T),
}

impl<'de, T> Deserialize<'de> for WireField<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct FieldVisitor<T>(std::marker::PhantomData<T>);

        impl<'de, T> serde::de::Visitor<'de> for FieldVisitor<T>
        where
            T: Deserialize<'de>,
        {
            type Value = WireField<T>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a Graph property value or null")
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(WireField::Null)
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(WireField::Null)
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                T::deserialize(deserializer).map(WireField::Value)
            }
        }

        deserializer.deserialize_option(FieldVisitor(std::marker::PhantomData))
    }
}

pub(super) struct WireRemoved {
    _reason: Option<String>,
}

impl<'de> Deserialize<'de> for WireRemoved {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RemovedFields {
            reason: Option<String>,
        }

        let removed = RemovedFields::deserialize(deserializer)?;
        Ok(Self {
            _reason: removed.reason,
        })
    }
}
