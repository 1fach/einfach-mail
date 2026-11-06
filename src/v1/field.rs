//! Shared tri-state for the writable fields of Graph resources,
//! distinguishing a field left out of a PATCH body from one explicitly
//! cleared. See the update semantics at
//! <https://learn.microsoft.com/en-us/graph/api/contact-update>.

use core::ops::Deref;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A writable Microsoft Graph resource field: absent from the
/// serialized body, explicitly null, or set to a value.
///
/// Graph PATCH semantics make the three states meaningful: an omitted
/// field keeps its stored value, a null one is cleared, a present one
/// is replaced. Collections clear through either `Null` or `Set` of an
/// empty collection (an explicit `[]` on the wire). Deserialization
/// maps a JSON null onto `Null` and a missing field onto `Unset` (via
/// the serde `default`).
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub enum MsgraphField<T> {
    /// Left out of the serialized body; an update preserves the stored
    /// value.
    #[default]
    Unset,
    /// Serialized as an explicit null; an update clears the stored
    /// value.
    Null,
    /// Serialized as the value itself.
    Set(T),
}

impl<T> MsgraphField<T> {
    /// True for the variant skipped by serialization.
    pub fn is_unset(&self) -> bool {
        matches!(self, Self::Unset)
    }

    /// The set value, `None` for both unset and null.
    pub fn as_option(&self) -> Option<&T> {
        match self {
            Self::Set(value) => Some(value),
            _ => None,
        }
    }

    /// The set value by value, `None` for both unset and null.
    pub fn into_option(self) -> Option<T> {
        match self {
            Self::Set(value) => Some(value),
            _ => None,
        }
    }

    /// `Set` for `Some`, `Null` for `None`.
    ///
    /// This is the natural encoding of a full-state update body, where
    /// every managed field is either replaced or cleared.
    pub fn set_or_null(value: Option<T>) -> Self {
        match value {
            Some(value) => Self::Set(value),
            None => Self::Null,
        }
    }
}

impl<T: Deref> MsgraphField<T> {
    /// The set value dereferenced (e.g. `&str` out of a `String`
    /// field), `None` for both unset and null.
    pub fn as_deref(&self) -> Option<&T::Target> {
        self.as_option().map(|value| value.deref())
    }
}

impl<T> From<T> for MsgraphField<T> {
    fn from(value: T) -> Self {
        Self::Set(value)
    }
}

impl<T: Serialize> Serialize for MsgraphField<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            // NOTE: Unset only reaches here without the paired
            // skip_serializing_if attribute; null is the least wrong
            // encoding then.
            Self::Unset | Self::Null => serializer.serialize_none(),
            Self::Set(value) => value.serialize(serializer),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for MsgraphField<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Option::<T>::deserialize(deserializer)? {
            Some(value) => Ok(Self::Set(value)),
            None => Ok(Self::Null),
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::{string::String, vec, vec::Vec};

    use serde::{Deserialize, Serialize};
    use serde_json::json;

    use crate::v1::field::MsgraphField;

    #[derive(Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
    struct Body {
        #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
        name: MsgraphField<String>,
        #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
        phones: MsgraphField<Vec<String>>,
    }

    #[test]
    fn unset_fields_are_omitted() {
        let body = serde_json::to_value(Body::default()).unwrap();
        assert_eq!(body, json!({}));
    }

    #[test]
    fn null_fields_serialize_as_explicit_null() {
        let body = Body {
            name: MsgraphField::Null,
            phones: MsgraphField::Null,
        };
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            json!({ "name": null, "phones": null })
        );
    }

    #[test]
    fn set_fields_serialize_as_values_including_empty_collections() {
        let body = Body {
            name: MsgraphField::Set(String::from("Jane")),
            phones: MsgraphField::Set(vec![]),
        };
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            json!({ "name": "Jane", "phones": [] })
        );
    }

    #[test]
    fn deserialization_distinguishes_missing_from_null() {
        let body: Body = serde_json::from_value(json!({ "name": null })).unwrap();
        assert_eq!(body.name, MsgraphField::Null);
        assert_eq!(body.phones, MsgraphField::Unset);

        let body: Body = serde_json::from_value(json!({ "phones": ["+33"] })).unwrap();
        assert_eq!(body.name, MsgraphField::Unset);
        assert_eq!(body.phones, MsgraphField::Set(vec![String::from("+33")]));
    }
}
