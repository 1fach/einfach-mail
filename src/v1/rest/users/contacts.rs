//! Microsoft Graph contacts (`users.contacts`): list, delta, get,
//! create, update, delete.
//!
//! <https://learn.microsoft.com/en-us/graph/api/resources/contact>

use alloc::{string::String, vec::Vec};

use serde::{Deserialize, Serialize};

use crate::v1::{field::MsgraphField, rest::users::messages::MsgraphEmailAddress};

pub mod create;
pub mod delete;
pub mod delta;
pub mod get;
pub mod list;
pub mod update;
#[cfg(feature = "vcard")]
pub mod vcard;

/// A contact in a contact folder. Doubles as the create/update body.
///
/// Unset fields are left out (an update preserves them), null fields
/// are serialized as explicit nulls (an update clears them), and a set
/// empty collection clears the collection.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MsgraphContact {
    /// The unique identifier of the contact.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// The display name of the contact.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub display_name: MsgraphField<String>,
    /// The given name of the contact.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub given_name: MsgraphField<String>,
    /// The middle name of the contact.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub middle_name: MsgraphField<String>,
    /// The family name of the contact.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub surname: MsgraphField<String>,
    /// The nickname of the contact.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub nick_name: MsgraphField<String>,
    /// The title of the contact.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub title: MsgraphField<String>,
    /// How the contact is filed under in the folder listing.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub file_as: MsgraphField<String>,
    /// The email addresses of the contact.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub email_addresses: MsgraphField<Vec<MsgraphEmailAddress>>,
    /// The instant messaging addresses of the contact.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub im_addresses: MsgraphField<Vec<String>>,
    /// The business phone numbers of the contact.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub business_phones: MsgraphField<Vec<String>>,
    /// The home phone numbers of the contact.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub home_phones: MsgraphField<Vec<String>>,
    /// The mobile phone number of the contact.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub mobile_phone: MsgraphField<String>,
    /// The home address of the contact.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub home_address: MsgraphField<MsgraphPhysicalAddress>,
    /// The business address of the contact.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub business_address: MsgraphField<MsgraphPhysicalAddress>,
    /// The alternative address of the contact.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub other_address: MsgraphField<MsgraphPhysicalAddress>,
    /// The job title of the contact.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub job_title: MsgraphField<String>,
    /// The name of the contact's company.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub company_name: MsgraphField<String>,
    /// The department the contact works in.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub department: MsgraphField<String>,
    /// The office location of the contact.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub office_location: MsgraphField<String>,
    /// The profession of the contact.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub profession: MsgraphField<String>,
    /// The name of the contact's assistant.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub assistant_name: MsgraphField<String>,
    /// The name of the contact's manager.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub manager: MsgraphField<String>,
    /// The business home page of the contact.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub business_home_page: MsgraphField<String>,
    /// Birthday as an ISO 8601 date-time (e.g. `1983-04-01T00:00:00Z`).
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub birthday: MsgraphField<String>,
    /// The name of the contact's spouse or partner.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub spouse_name: MsgraphField<String>,
    /// The names of the contact's children.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub children: MsgraphField<Vec<String>>,
    /// The free-form notes about the contact.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub personal_notes: MsgraphField<String>,
    /// The categories associated with the contact.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub categories: MsgraphField<Vec<String>>,
    /// The identifier of the containing contact folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_folder_id: Option<String>,
    /// The creation date, as an ISO 8601 date-time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_date_time: Option<String>,
    /// The last modification date, as an ISO 8601 date-time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified_date_time: Option<String>,
    /// The version marker of the contact, changing on every update.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_key: Option<String>,
    /// Single-value extended MAPI properties attached to the contact.
    ///
    /// They ride inline in create and update bodies, but responses
    /// only carry them when the request `$expand`ed them by id.
    #[serde(default, skip_serializing_if = "MsgraphField::is_unset")]
    pub single_value_extended_properties: MsgraphField<Vec<MsgraphSingleValueExtendedProperty>>,
}

/// A physical address of a contact (home, business or other).
///
/// The whole object is replaced at once, so its components stay plain
/// options.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MsgraphPhysicalAddress {
    /// The street name and number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub street: Option<String>,
    /// The city.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    /// The state or province.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// The country or region.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country_or_region: Option<String>,
    /// The postal code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postal_code: Option<String>,
}

/// A single-value extended MAPI property: the full property id (the
/// `String {guid} Name <name>` form) and its value.
///
/// <https://learn.microsoft.com/en-us/graph/api/resources/singlevaluelegacyextendedproperty>
#[derive(Debug, Clone, Default, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct MsgraphSingleValueExtendedProperty {
    /// The full property identifier.
    #[serde(default)]
    pub id: String,
    /// The value of the property.
    #[serde(default)]
    pub value: String,
}
