use quick_xml::{
    NsReader,
    events::{BytesStart, Event},
    name::ResolveResult,
};
use url::Url;

use crate::{error::EwsError, transport::parse_endpoint};

pub(super) const MAX_XML_DEPTH: usize = 64;
pub(super) const REQUEST_NAMESPACE: &str =
    "http://schemas.microsoft.com/exchange/autodiscover/outlook/requestschema/2006";
pub(super) const RESPONSE_NAMESPACE: &str =
    "http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006";
pub(super) const ACCEPTABLE_RESPONSE_SCHEMA: &str =
    "http://schemas.microsoft.com/exchange/autodiscover/outlook/responseschema/2006a";

pub(super) enum PoxResponse {
    Endpoint(Url),
    RedirectUrl(Url),
    RedirectAddress(String),
    NoEndpoint,
}

enum ProtocolText {
    Type,
    EwsUrl,
}

enum PoxTextTarget {
    RedirectUrl,
    RedirectAddress,
    Protocol(ProtocolText),
}

#[derive(Default)]
struct ProtocolEndpoint {
    protocol_type: Option<String>,
    ews_url: Option<String>,
}

#[derive(Default)]
struct PoxParser {
    stack: Vec<String>,
    root_namespace: Option<Vec<u8>>,
    root_closed: bool,
    redirect_url: Option<String>,
    redirect_address: Option<String>,
    protocols: Vec<ProtocolEndpoint>,
    current_protocol: Option<(usize, ProtocolEndpoint)>,
    text_buffer: Option<(PoxTextTarget, String)>,
}

pub(super) fn parse_pox_response(document: &[u8]) -> Result<PoxResponse, EwsError> {
    PoxParser::parse(document)
}

impl PoxParser {
    fn parse(document: &[u8]) -> Result<PoxResponse, EwsError> {
        let mut reader = NsReader::from_reader(document);
        reader.config_mut().trim_text(false);
        let decoder = reader.decoder();
        let mut parser = Self::default();

        loop {
            let (namespace, event) = reader
                .read_resolved_event()
                .map_err(|_| EwsError::AutodiscoverFailed)?;
            if matches!(event, Event::Eof) {
                break;
            }
            parser.accept(namespace, event, decoder)?;
        }
        parser.finish()
    }

    fn accept(
        &mut self,
        namespace: ResolveResult<'_>,
        event: Event<'_>,
        decoder: quick_xml::encoding::Decoder,
    ) -> Result<(), EwsError> {
        match event {
            Event::Start(element) => self.start(namespace, &element, decoder),
            Event::Empty(element) => self.empty(namespace, &element, decoder),
            Event::Text(text) => self.text(&text),
            Event::CData(text) => self.cdata(&text),
            Event::End(element) => {
                self.validate_namespace(namespace)?;
                self.end(&element)
            }
            Event::DocType(_) => Err(EwsError::AutodiscoverFailed),
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) | Event::Eof => Ok(()),
            Event::GeneralRef(reference) => self.general_reference(&reference),
        }
    }

    fn start(
        &mut self,
        namespace: ResolveResult<'_>,
        element: &BytesStart<'_>,
        decoder: quick_xml::encoding::Decoder,
    ) -> Result<(), EwsError> {
        if self.text_buffer.is_some() {
            return Err(EwsError::AutodiscoverFailed);
        }
        let name = local_name(element);
        if self.stack.is_empty() && name != "Autodiscover" {
            return Err(EwsError::AutodiscoverFailed);
        }
        self.validate_start(namespace)?;
        self.begin_protocol(&name, element, decoder)?;
        self.stack.push(name);
        self.begin_text();
        Ok(())
    }

    fn validate_start(&mut self, namespace: ResolveResult<'_>) -> Result<(), EwsError> {
        if self.root_closed {
            return Err(EwsError::AutodiscoverFailed);
        }
        if self.stack.len() >= MAX_XML_DEPTH {
            return Err(EwsError::ResourceLimitExceeded);
        }
        if self.stack.is_empty() {
            self.set_root_namespace(namespace)
        } else {
            self.validate_namespace(namespace)
        }
    }

    fn validate_namespace(&self, namespace: ResolveResult<'_>) -> Result<(), EwsError> {
        if namespace_matches(namespace, self.root_namespace.as_deref()) {
            Ok(())
        } else {
            Err(EwsError::AutodiscoverFailed)
        }
    }

    fn set_root_namespace(&mut self, namespace: ResolveResult<'_>) -> Result<(), EwsError> {
        if self.root_closed {
            return Err(EwsError::AutodiscoverFailed);
        }
        match namespace {
            ResolveResult::Bound(namespace)
                if namespace.as_ref() == RESPONSE_NAMESPACE.as_bytes() =>
            {
                self.root_namespace = Some(namespace.as_ref().to_vec());
                Ok(())
            }
            _ => Err(EwsError::AutodiscoverFailed),
        }
    }

    fn begin_protocol(
        &mut self,
        name: &str,
        element: &BytesStart<'_>,
        decoder: quick_xml::encoding::Decoder,
    ) -> Result<(), EwsError> {
        if name == "Protocol" && path_is(&self.stack, &["Autodiscover", "Response", "Account"]) {
            self.current_protocol =
                Some((self.stack.len() + 1, protocol_from_start(element, decoder)?));
        }
        Ok(())
    }

    fn empty(
        &mut self,
        namespace: ResolveResult<'_>,
        element: &BytesStart<'_>,
        decoder: quick_xml::encoding::Decoder,
    ) -> Result<(), EwsError> {
        if self.text_buffer.is_some() {
            return Err(EwsError::AutodiscoverFailed);
        }
        if self.stack.is_empty() || !namespace_matches(namespace, self.root_namespace.as_deref()) {
            return Err(EwsError::AutodiscoverFailed);
        }
        if self.stack.len() >= MAX_XML_DEPTH {
            return Err(EwsError::ResourceLimitExceeded);
        }
        self.add_empty_protocol(element, decoder)
    }

    fn add_empty_protocol(
        &mut self,
        element: &BytesStart<'_>,
        decoder: quick_xml::encoding::Decoder,
    ) -> Result<(), EwsError> {
        if local_name(element) == "Protocol"
            && path_is(&self.stack, &["Autodiscover", "Response", "Account"])
        {
            self.protocols.push(protocol_from_start(element, decoder)?);
        }
        Ok(())
    }

    fn text(&mut self, text: &quick_xml::events::BytesText<'_>) -> Result<(), EwsError> {
        let decoded = text.decode().map_err(|_| EwsError::AutodiscoverFailed)?;
        let unescaped =
            quick_xml::escape::unescape(&decoded).map_err(|_| EwsError::AutodiscoverFailed)?;
        self.append_text(&unescaped)
    }

    fn general_reference(
        &mut self,
        reference: &quick_xml::events::BytesRef<'_>,
    ) -> Result<(), EwsError> {
        if self.stack.is_empty() {
            return Err(EwsError::AutodiscoverFailed);
        }
        let reference_text = decode_xml_reference(reference)?;
        self.append_text(&reference_text)
    }

    fn append_text(&mut self, text_chunk: &str) -> Result<(), EwsError> {
        if let Some((_, text)) = &mut self.text_buffer {
            text.push_str(text_chunk);
            return Ok(());
        }
        if self.stack.is_empty() && !text_chunk.trim().is_empty() {
            return Err(EwsError::AutodiscoverFailed);
        }
        Ok(())
    }

    fn cdata(&mut self, text: &quick_xml::events::BytesCData<'_>) -> Result<(), EwsError> {
        if self.stack.is_empty() {
            return Err(EwsError::AutodiscoverFailed);
        }
        let decoded_text = text.decode().map_err(|_| EwsError::AutodiscoverFailed)?;
        self.append_text(&decoded_text)
    }

    fn end(&mut self, element: &quick_xml::events::BytesEnd<'_>) -> Result<(), EwsError> {
        let name = String::from_utf8_lossy(element.local_name().as_ref()).into_owned();
        self.finish_text()?;
        if self.stack.pop().as_deref() != Some(name.as_str()) {
            return Err(EwsError::AutodiscoverFailed);
        }
        self.finish_protocol();
        if self.stack.is_empty() {
            self.root_closed = true;
        }
        Ok(())
    }

    fn begin_text(&mut self) {
        if let Some(target) = pox_text_target(&self.stack) {
            self.text_buffer = Some((target, String::new()));
        }
    }

    fn finish_text(&mut self) -> Result<(), EwsError> {
        let Some((target, text)) = self.text_buffer.take() else {
            return Ok(());
        };
        let field_text = text.trim().to_owned();
        if field_text.is_empty() {
            return Ok(());
        }
        match target {
            PoxTextTarget::RedirectUrl => set_once(&mut self.redirect_url, field_text),
            PoxTextTarget::RedirectAddress => set_once(&mut self.redirect_address, field_text),
            PoxTextTarget::Protocol(ProtocolText::Type) => self.set_protocol_type(field_text),
            PoxTextTarget::Protocol(ProtocolText::EwsUrl) => self.set_protocol_ews_url(field_text),
        }
    }

    fn set_protocol_type(&mut self, protocol_type: String) -> Result<(), EwsError> {
        let Some((depth, protocol)) = &mut self.current_protocol else {
            return Ok(());
        };
        if *depth != self.stack.len() - 1 {
            return Ok(());
        }
        set_once(&mut protocol.protocol_type, protocol_type)
    }

    fn set_protocol_ews_url(&mut self, endpoint: String) -> Result<(), EwsError> {
        let Some((depth, protocol)) = &mut self.current_protocol else {
            return Ok(());
        };
        if *depth != self.stack.len() - 1 {
            return Ok(());
        }
        set_once(&mut protocol.ews_url, endpoint)
    }

    fn finish_protocol(&mut self) {
        if self
            .current_protocol
            .as_ref()
            .is_some_and(|(depth, _)| *depth == self.stack.len() + 1)
            && let Some((_, protocol)) = self.current_protocol.take()
        {
            self.protocols.push(protocol);
        }
    }

    fn finish(self) -> Result<PoxResponse, EwsError> {
        if !self.root_closed || !self.stack.is_empty() {
            return Err(EwsError::AutodiscoverFailed);
        }
        if let Some(address) = self.redirect_address {
            return Ok(PoxResponse::RedirectAddress(address));
        }
        if let Some(url) = self.redirect_url {
            return Ok(PoxResponse::RedirectUrl(parse_endpoint(&url)?));
        }
        self.endpoint()
    }

    fn endpoint(&self) -> Result<PoxResponse, EwsError> {
        for preferred_type in ["EXPR", "EXCH"] {
            for protocol in &self.protocols {
                if protocol.protocol_type.as_deref() == Some(preferred_type)
                    && let Some(endpoint) = &protocol.ews_url
                {
                    return Ok(PoxResponse::Endpoint(parse_endpoint(endpoint)?));
                }
            }
        }
        Ok(PoxResponse::NoEndpoint)
    }
}

fn local_name(element: &BytesStart<'_>) -> String {
    String::from_utf8_lossy(element.local_name().as_ref()).into_owned()
}

fn pox_text_target(path: &[String]) -> Option<PoxTextTarget> {
    if path_is(
        path,
        &["Autodiscover", "Response", "Account", "RedirectUrl"],
    ) {
        Some(PoxTextTarget::RedirectUrl)
    } else if path_is(
        path,
        &["Autodiscover", "Response", "Account", "RedirectAddr"],
    ) {
        Some(PoxTextTarget::RedirectAddress)
    } else {
        protocol_text_kind(path).map(PoxTextTarget::Protocol)
    }
}

fn namespace_matches(namespace: ResolveResult<'_>, expected: Option<&[u8]>) -> bool {
    matches!(namespace, ResolveResult::Bound(bound_namespace) if Some(bound_namespace.as_ref()) == expected)
}

fn path_is(path: &[String], expected: &[&str]) -> bool {
    path.len() == expected.len()
        && path
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual == expected)
}

fn protocol_text_kind(path: &[String]) -> Option<ProtocolText> {
    if path_is(
        path,
        &["Autodiscover", "Response", "Account", "Protocol", "Type"],
    ) {
        Some(ProtocolText::Type)
    } else if path_is(
        path,
        &["Autodiscover", "Response", "Account", "Protocol", "ASUrl"],
    ) || path_is(
        path,
        &["Autodiscover", "Response", "Account", "Protocol", "EwsUrl"],
    ) {
        Some(ProtocolText::EwsUrl)
    } else {
        None
    }
}

fn is_xml_character(character: char) -> bool {
    matches!(
        character as u32,
        0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF
    )
}

fn decode_xml_reference(reference: &quick_xml::events::BytesRef<'_>) -> Result<String, EwsError> {
    let character = reference
        .resolve_char_ref()
        .map_err(|_| EwsError::AutodiscoverFailed)?;
    if let Some(character) = character {
        return is_xml_character(character)
            .then(|| character.to_string())
            .ok_or(EwsError::AutodiscoverFailed);
    }
    let name = reference
        .decode()
        .map_err(|_| EwsError::AutodiscoverFailed)?;
    quick_xml::escape::resolve_xml_entity(&name)
        .map(str::to_owned)
        .ok_or(EwsError::AutodiscoverFailed)
}

fn protocol_from_start(
    element: &BytesStart<'_>,
    decoder: quick_xml::encoding::Decoder,
) -> Result<ProtocolEndpoint, EwsError> {
    let mut protocol = ProtocolEndpoint::default();
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute.map_err(|_| EwsError::AutodiscoverFailed)?;
        if attribute.key.local_name().as_ref() == b"Type" {
            let protocol_type = attribute
                .decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, decoder)
                .map_err(|_| EwsError::AutodiscoverFailed)?
                .into_owned();
            set_once(&mut protocol.protocol_type, protocol_type)?;
        }
    }
    Ok(protocol)
}

fn set_once(slot: &mut Option<String>, new_value: String) -> Result<(), EwsError> {
    if slot.as_ref().is_some_and(|current| current != &new_value) {
        return Err(EwsError::AutodiscoverFailed);
    }
    *slot = Some(new_value);
    Ok(())
}
