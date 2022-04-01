use std::fmt;

use crate::{
    crypto::{CryptoKeyPair, KeyPair},
    error::Error,
    tlv::{self, FromTLV, TLVArrayOwned, TLVElement, TLVWriter, TagType, ToTLV},
    utils::writebuf::WriteBuf,
};
use log::error;
use num_derive::FromPrimitive;

use self::{asn1_writer::ASN1Writer, printer::CertPrinter};

// As per https://datatracker.ietf.org/doc/html/rfc5280

const OID_PUB_KEY_ECPUBKEY: [u8; 7] = [0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x02, 0x01];
const OID_EC_TYPE_PRIME256V1: [u8; 8] = [0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07];
const OID_ECDSA_WITH_SHA256: [u8; 8] = [0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04, 0x03, 0x02];

#[derive(FromPrimitive)]
pub enum CertTags {
    SerialNum = 1,
    SignAlgo = 2,
    Issuer = 3,
    NotBefore = 4,
    NotAfter = 5,
    Subject = 6,
    PubKeyAlgo = 7,
    EcCurveId = 8,
    EcPubKey = 9,
    Extensions = 10,
    Signature = 11,
}

#[derive(FromPrimitive, Debug)]
pub enum EcCurveIdValue {
    Prime256V1 = 1,
}

pub fn get_ec_curve_id(algo: u8) -> Option<EcCurveIdValue> {
    num::FromPrimitive::from_u8(algo)
}

#[derive(FromPrimitive, Debug)]
pub enum PubKeyAlgoValue {
    EcPubKey = 1,
}

pub fn get_pubkey_algo(algo: u8) -> Option<PubKeyAlgoValue> {
    num::FromPrimitive::from_u8(algo)
}

#[derive(FromPrimitive, Debug)]
pub enum SignAlgoValue {
    ECDSAWithSHA256 = 1,
}

pub fn get_sign_algo(algo: u8) -> Option<SignAlgoValue> {
    num::FromPrimitive::from_u8(algo)
}

const KEY_USAGE_DIGITAL_SIGN: u16 = 0x0001;
const KEY_USAGE_NON_REPUDIATION: u16 = 0x0002;
const KEY_USAGE_KEY_ENCIPHERMENT: u16 = 0x0004;
const KEY_USAGE_DATA_ENCIPHERMENT: u16 = 0x0008;
const KEY_USAGE_KEY_AGREEMENT: u16 = 0x0010;
const KEY_USAGE_KEY_CERT_SIGN: u16 = 0x0020;
const KEY_USAGE_CRL_SIGN: u16 = 0x0040;
const KEY_USAGE_ENCIPHER_ONLY: u16 = 0x0080;
const KEY_USAGE_DECIPHER_ONLY: u16 = 0x0100;

fn reverse_byte(byte: u8) -> u8 {
    const LOOKUP: [u8; 16] = [
        0x00, 0x08, 0x04, 0x0c, 0x02, 0x0a, 0x06, 0x0e, 0x01, 0x09, 0x05, 0x0d, 0x03, 0x0b, 0x07,
        0x0f,
    ];
    (LOOKUP[(byte & 0x0f) as usize] << 4) | LOOKUP[(byte >> 4) as usize]
}

fn int_to_bitstring(mut a: u16, buf: &mut [u8]) {
    if buf.len() >= 2 {
        buf[0] = reverse_byte((a & 0xff) as u8);
        a >>= 8;
        buf[1] = reverse_byte((a & 0xff) as u8);
    }
}

macro_rules! add_if {
    ($key:ident, $bit:ident,$str:literal) => {
        if ($key & $bit) != 0 {
            $str
        } else {
            ""
        }
    };
}

fn get_print_str(key_usage: u16) -> String {
    format!(
        "{}{}{}{}{}{}{}{}{}",
        add_if!(key_usage, KEY_USAGE_DIGITAL_SIGN, "digitalSignature "),
        add_if!(key_usage, KEY_USAGE_NON_REPUDIATION, "nonRepudiation "),
        add_if!(key_usage, KEY_USAGE_KEY_ENCIPHERMENT, "keyEncipherment "),
        add_if!(key_usage, KEY_USAGE_DATA_ENCIPHERMENT, "dataEncipherment "),
        add_if!(key_usage, KEY_USAGE_KEY_AGREEMENT, "keyAgreement "),
        add_if!(key_usage, KEY_USAGE_KEY_CERT_SIGN, "keyCertSign "),
        add_if!(key_usage, KEY_USAGE_CRL_SIGN, "CRLSign "),
        add_if!(key_usage, KEY_USAGE_ENCIPHER_ONLY, "encipherOnly "),
        add_if!(key_usage, KEY_USAGE_DECIPHER_ONLY, "decipherOnly "),
    )
}

#[allow(unused_assignments)]
fn encode_key_usage(key_usage: u16, w: &mut dyn CertConsumer) -> Result<(), Error> {
    let mut key_usage_str = [0u8; 2];
    int_to_bitstring(key_usage, &mut key_usage_str);
    w.bitstr(&get_print_str(key_usage), true, &key_usage_str)?;
    Ok(())
}

fn encode_extended_key_usage(
    list: &TLVArrayOwned<u8>,
    w: &mut dyn CertConsumer,
) -> Result<(), Error> {
    const OID_SERVER_AUTH: [u8; 8] = [0x2B, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x01];
    const OID_CLIENT_AUTH: [u8; 8] = [0x2B, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x02];
    const OID_CODE_SIGN: [u8; 8] = [0x2B, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x03];
    const OID_EMAIL_PROT: [u8; 8] = [0x2B, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x04];
    const OID_TIMESTAMP: [u8; 8] = [0x2B, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x08];
    const OID_OCSP_SIGN: [u8; 8] = [0x2B, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x09];
    let encoding = [
        ("", &[0; 8]),
        ("ServerAuth", &OID_SERVER_AUTH),
        ("ClientAuth", &OID_CLIENT_AUTH),
        ("CodeSign", &OID_CODE_SIGN),
        ("EmailProtection", &OID_EMAIL_PROT),
        ("Timestamp", &OID_TIMESTAMP),
        ("OCSPSign", &OID_OCSP_SIGN),
    ];

    w.start_seq("")?;
    for t in list.iter() {
        let t = *t as usize;
        if t > 0 && t <= encoding.len() {
            w.oid(encoding[t].0, encoding[t].1)?;
        } else {
            error!("Skipping encoding key usage out of bounds");
        }
    }
    w.end_seq()?;
    Ok(())
}

#[derive(FromTLV, ToTLV, Default)]
#[tlvargs(start = 1)]
struct BasicConstraints {
    is_ca: bool,
    path: Option<u8>,
}

impl BasicConstraints {
    pub fn encode(&self, w: &mut dyn CertConsumer) -> Result<(), Error> {
        w.start_seq("")?;
        if self.is_ca {
            // Encode CA only if true
            w.bool("CA:", true)?
        }
        if self.path.is_some() {
            error!("Path Len is not yet implemented");
        }
        w.end_seq()
    }
}

fn encode_extension_start(
    tag: &str,
    critical: bool,
    oid: &[u8],
    w: &mut dyn CertConsumer,
) -> Result<(), Error> {
    w.start_seq(tag)?;
    w.oid("", oid)?;
    if critical {
        w.bool("critical:", true)?;
    }
    w.start_compound_ostr("value:")
}

fn encode_extension_end(w: &mut dyn CertConsumer) -> Result<(), Error> {
    w.end_compound_ostr()?;
    w.end_seq()
}

#[derive(FromTLV, ToTLV, Default)]
#[tlvargs(start = 1, datatype = "list")]
struct Extensions {
    basic_const: Option<BasicConstraints>,
    key_usage: Option<u16>,
    ext_key_usage: Option<TLVArrayOwned<u8>>,
    subj_key_id: Option<Vec<u8>>,
    auth_key_id: Option<Vec<u8>>,
    future_extensions: Option<Vec<u8>>,
}

impl Extensions {
    fn encode(&self, w: &mut dyn CertConsumer) -> Result<(), Error> {
        const OID_BASIC_CONSTRAINTS: [u8; 3] = [0x55, 0x1D, 0x13];
        const OID_KEY_USAGE: [u8; 3] = [0x55, 0x1D, 0x0F];
        const OID_EXT_KEY_USAGE: [u8; 3] = [0x55, 0x1D, 0x25];
        const OID_SUBJ_KEY_IDENTIFIER: [u8; 3] = [0x55, 0x1D, 0x0E];
        const OID_AUTH_KEY_ID: [u8; 3] = [0x55, 0x1D, 0x23];

        w.start_ctx("X509v3 extensions:", 3)?;
        w.start_seq("")?;
        if let Some(t) = &self.basic_const {
            encode_extension_start("X509v3 Basic Constraints", true, &OID_BASIC_CONSTRAINTS, w)?;
            t.encode(w)?;
            encode_extension_end(w)?;
        }
        if let Some(t) = self.key_usage {
            encode_extension_start("X509v3 Key Usage", true, &OID_KEY_USAGE, w)?;
            encode_key_usage(t, w)?;
            encode_extension_end(w)?;
        }
        if let Some(t) = &self.ext_key_usage {
            encode_extension_start("X509v3 Extended Key Usage", true, &OID_EXT_KEY_USAGE, w)?;
            encode_extended_key_usage(&t, w)?;
            encode_extension_end(w)?;
        }
        if let Some(t) = &self.subj_key_id {
            encode_extension_start("Subject Key ID", false, &OID_SUBJ_KEY_IDENTIFIER, w)?;
            w.ostr("", t.as_slice())?;
            encode_extension_end(w)?;
        }
        if let Some(t) = &self.auth_key_id {
            encode_extension_start("Auth Key ID", false, &OID_AUTH_KEY_ID, w)?;
            w.start_seq("")?;
            w.ctx("", 0, t.as_slice())?;
            w.end_seq()?;
            encode_extension_end(w)?;
        }
        if let Some(t) = &self.future_extensions {
            error!("Future Extensions Not Yet Supported: {:x?}", t.as_slice())
        }
        w.end_seq()?;
        w.end_ctx()?;
        Ok(())
    }
}
const MAX_DN_ENTRIES: usize = 5;

#[derive(FromPrimitive, Copy, Clone)]
enum DnTags {
    NodeId = 17,
    FirmwareSignId = 18,
    IcaId = 19,
    RootCaId = 20,
    FabricId = 21,
    NocCat = 22,
}

#[derive(Default)]
struct DistNames {
    // The order in which the DNs arrive is important, as the signing
    // requires that the ASN1 notation retains the same order
    dn: Vec<(u8, u64)>,
}

impl DistNames {
    fn u64(&self, match_id: DnTags) -> Option<u64> {
        self.dn
            .iter()
            .find(|(id, _)| *id == match_id as u8)
            .map(|(_, value)| *value)
    }
}

impl<'a> FromTLV<'a> for DistNames {
    fn from_tlv(t: &TLVElement<'a>) -> Result<Self, Error> {
        let mut d = Self {
            dn: Vec::with_capacity(MAX_DN_ENTRIES),
        };
        let iter = t.confirm_list()?.iter().ok_or(Error::Invalid)?;
        for t in iter {
            if let TagType::Context(tag) = t.get_tag() {
                let value = t.u64().map_err(|e| {
                    // Non-integer DNs not yet supported
                    error!("This DN is not yet supported{}", tag);
                    e
                })?;
                d.dn.push((tag, value));
            }
        }
        Ok(d)
    }
}

impl ToTLV for DistNames {
    fn to_tlv(&self, tw: &mut TLVWriter, tag: TagType) -> Result<(), Error> {
        tw.start_list(tag)?;
        for (name, value) in &self.dn {
            tw.u64(TagType::Context(*name), *value)?;
        }
        tw.end_container()
    }
}

impl DistNames {
    fn encode(&self, tag: &str, w: &mut dyn CertConsumer) -> Result<(), Error> {
        const OID_MATTER_NODE_ID: [u8; 10] =
            [0x2B, 0x06, 0x01, 0x04, 0x01, 0x82, 0xA2, 0x7C, 0x01, 0x01];
        const OID_MATTER_FW_SIGN_ID: [u8; 10] =
            [0x2B, 0x06, 0x01, 0x04, 0x01, 0x82, 0xA2, 0x7C, 0x01, 0x02];
        const OID_MATTER_ICA_ID: [u8; 10] =
            [0x2B, 0x06, 0x01, 0x04, 0x01, 0x82, 0xA2, 0x7C, 0x01, 0x03];
        const OID_MATTER_ROOT_CA_ID: [u8; 10] =
            [0x2B, 0x06, 0x01, 0x04, 0x01, 0x82, 0xA2, 0x7C, 0x01, 0x04];
        const OID_MATTER_FABRIC_ID: [u8; 10] =
            [0x2B, 0x06, 0x01, 0x04, 0x01, 0x82, 0xA2, 0x7C, 0x01, 0x05];
        const OID_MATTER_NOC_CAT_ID: [u8; 10] =
            [0x2B, 0x06, 0x01, 0x04, 0x01, 0x82, 0xA2, 0x7C, 0x01, 0x06];

        let dn_encoding = [
            ("Chip Node Id:", &OID_MATTER_NODE_ID),
            ("Chip Firmware Signing Id:", &OID_MATTER_FW_SIGN_ID),
            ("Chip ICA Id:", &OID_MATTER_ICA_ID),
            ("Chip Root CA Id:", &OID_MATTER_ROOT_CA_ID),
            ("Chip Fabric Id:", &OID_MATTER_FABRIC_ID),
        ];

        w.start_seq(tag)?;
        for (id, value) in &self.dn {
            if let Ok(tag) = num::FromPrimitive::from_u8(*id).ok_or(Error::InvalidData) {
                match tag {
                    DnTags::NocCat => {
                        w.start_set("")?;
                        w.start_seq("")?;
                        w.oid("Chip NOC CAT Id:", &OID_MATTER_NOC_CAT_ID)?;
                        w.utf8str("", format!("{:08X}", value).as_str())?;
                        w.end_seq()?;
                        w.end_set()?;
                    }
                    _ => {
                        let index: usize = (*id as usize) - (DnTags::NodeId as usize);
                        let this = &dn_encoding[index];
                        encode_u64_dn(*value, this.0, this.1, w)?;
                    }
                }
            } else {
                error!("Non Matter DNs are not yet supported {}", id);
            }
        }
        w.end_seq()?;
        Ok(())
    }
}

fn encode_u64_dn(
    value: u64,
    name: &str,
    oid: &[u8],
    w: &mut dyn CertConsumer,
) -> Result<(), Error> {
    w.start_set("")?;
    w.start_seq("")?;
    w.oid(name, oid)?;
    w.utf8str("", format!("{:016X}", value).as_str())?;
    w.end_seq()?;
    w.end_set()
}

#[derive(FromTLV, ToTLV, Default)]
#[tlvargs(start = 1)]
pub struct Cert {
    serial_no: Vec<u8>,
    sign_algo: u8,
    issuer: DistNames,
    not_before: u32,
    not_after: u32,
    subject: DistNames,
    pubkey_algo: u8,
    ec_curve_id: u8,
    pubkey: Vec<u8>,
    extensions: Extensions,
    signature: Vec<u8>,
}

// TODO: Instead of parsing the TLVs everytime, we should just cache this, but the encoding
// rules in terms of sequence may get complicated. Need to look into this
impl Cert {
    pub fn new(cert_bin: &[u8]) -> Result<Self, Error> {
        let root = tlv::get_root_node(cert_bin)?;
        Cert::from_tlv(&root)
    }

    pub fn get_node_id(&self) -> Result<u64, Error> {
        self.subject.u64(DnTags::NodeId).ok_or(Error::NoNodeId)
    }

    pub fn get_fabric_id(&self) -> Result<u64, Error> {
        self.subject.u64(DnTags::FabricId).ok_or(Error::NoFabricId)
    }

    pub fn get_pubkey(&self) -> &[u8] {
        self.pubkey.as_slice()
    }

    pub fn get_subject_key_id(&self) -> Result<&[u8], Error> {
        self.extensions
            .subj_key_id
            .as_ref()
            .map(|x| x.as_slice())
            .ok_or(Error::Invalid)
    }

    pub fn is_authority(&self, their: &Cert) -> Result<bool, Error> {
        if let Some(our_auth_key) = &self.extensions.auth_key_id {
            let their_subject = their.get_subject_key_id()?;
            if our_auth_key == their_subject {
                Ok(true)
            } else {
                Ok(false)
            }
        } else {
            Ok(false)
        }
    }

    pub fn get_signature(&self) -> &[u8] {
        self.signature.as_slice()
    }

    pub fn as_tlv(&self, buf: &mut [u8]) -> Result<usize, Error> {
        let mut wb = WriteBuf::new(buf, buf.len());
        let mut tw = TLVWriter::new(&mut wb);
        self.to_tlv(&mut tw, TagType::Anonymous)?;
        Ok(wb.as_slice().len())
    }

    pub fn as_asn1(&self, buf: &mut [u8]) -> Result<usize, Error> {
        let mut w = ASN1Writer::new(buf);
        let _ = self.encode(&mut w)?;
        Ok(w.as_slice().len())
    }

    pub fn verify_chain_start(&self) -> CertVerifier {
        CertVerifier::new(self)
    }

    fn encode(&self, w: &mut dyn CertConsumer) -> Result<(), Error> {
        w.start_seq("")?;

        w.start_ctx("Version:", 0)?;
        w.integer("", &[2])?;
        w.end_ctx()?;

        w.integer("Serial Num:", self.serial_no.as_slice())?;

        w.start_seq("Signature Algorithm:")?;
        let (str, oid) = match get_sign_algo(self.sign_algo).ok_or(Error::Invalid)? {
            SignAlgoValue::ECDSAWithSHA256 => ("ECDSA with SHA256", OID_ECDSA_WITH_SHA256),
        };
        w.oid(str, &oid)?;
        w.end_seq()?;

        self.issuer.encode("Issuer:", w)?;

        w.start_seq("Validity:")?;
        w.utctime("Not Before:", self.not_before)?;
        w.utctime("Not After:", self.not_after)?;
        w.end_seq()?;

        self.subject.encode("Subject:", w)?;

        w.start_seq("")?;
        w.start_seq("Public Key Algorithm")?;
        let (str, pub_key) = match get_pubkey_algo(self.pubkey_algo).ok_or(Error::Invalid)? {
            PubKeyAlgoValue::EcPubKey => ("ECPubKey", OID_PUB_KEY_ECPUBKEY),
        };
        w.oid(str, &pub_key)?;
        let (str, curve_id) = match get_ec_curve_id(self.ec_curve_id).ok_or(Error::Invalid)? {
            EcCurveIdValue::Prime256V1 => ("Prime256v1", OID_EC_TYPE_PRIME256V1),
        };
        w.oid(str, &curve_id)?;
        w.end_seq()?;

        w.bitstr("Public-Key:", false, self.pubkey.as_slice())?;
        w.end_seq()?;

        self.extensions.encode(w)?;

        // We do not encode the Signature in the DER certificate

        w.end_seq()
    }
}

impl fmt::Display for Cert {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut printer = CertPrinter::new(f);
        let _ = self
            .encode(&mut printer)
            .map_err(|e| error!("Error decoding certificate: {}", e));
        // Signature is not encoded by the Cert Decoder
        writeln!(f, "Signature: {:x?}", self.get_signature())
    }
}

pub struct CertVerifier<'a> {
    cert: &'a Cert,
}

impl<'a> CertVerifier<'a> {
    pub fn new(cert: &'a Cert) -> Self {
        Self { cert }
    }

    pub fn add_cert(self, parent: &'a Cert) -> Result<CertVerifier<'a>, Error> {
        if !self.cert.is_authority(parent)? {
            return Err(Error::InvalidAuthKey);
        }
        let mut asn1 = [0u8; MAX_ASN1_CERT_SIZE];
        let len = self.cert.as_asn1(&mut asn1)?;
        let asn1 = &asn1[..len];

        let k = KeyPair::new_from_public(parent.get_pubkey())?;
        k.verify_msg(asn1, self.cert.get_signature()).map_err(|e| {
            error!(
                "Error in signature verification of certificate: {:#02x?}",
                self.cert.get_subject_key_id()
            );
            e
        })?;

        // TODO: other validation checks
        Ok(CertVerifier::new(parent))
    }

    pub fn finalise(self) -> Result<(), Error> {
        let cert = self.cert;
        self.add_cert(cert)?;
        Ok(())
    }
}

pub trait CertConsumer {
    fn start_seq(&mut self, tag: &str) -> Result<(), Error>;
    fn end_seq(&mut self) -> Result<(), Error>;
    fn integer(&mut self, tag: &str, i: &[u8]) -> Result<(), Error>;
    fn utf8str(&mut self, tag: &str, s: &str) -> Result<(), Error>;
    fn bitstr(&mut self, tag: &str, truncate: bool, s: &[u8]) -> Result<(), Error>;
    fn ostr(&mut self, tag: &str, s: &[u8]) -> Result<(), Error>;
    fn start_compound_ostr(&mut self, tag: &str) -> Result<(), Error>;
    fn end_compound_ostr(&mut self) -> Result<(), Error>;
    fn bool(&mut self, tag: &str, b: bool) -> Result<(), Error>;
    fn start_set(&mut self, tag: &str) -> Result<(), Error>;
    fn end_set(&mut self) -> Result<(), Error>;
    fn ctx(&mut self, tag: &str, id: u8, val: &[u8]) -> Result<(), Error>;
    fn start_ctx(&mut self, tag: &str, id: u8) -> Result<(), Error>;
    fn end_ctx(&mut self) -> Result<(), Error>;
    fn oid(&mut self, tag: &str, oid: &[u8]) -> Result<(), Error>;
    fn utctime(&mut self, tag: &str, epoch: u32) -> Result<(), Error>;
}

const MAX_DEPTH: usize = 10;
const MAX_ASN1_CERT_SIZE: usize = 800;

mod asn1_writer;
mod printer;

#[cfg(test)]
mod tests {
    use crate::cert::Cert;
    use crate::error::Error;
    use crate::tlv::{self, FromTLV, TLVWriter, TagType, ToTLV};
    use crate::utils::writebuf::WriteBuf;

    #[test]
    fn test_asn1_encode_success() {
        {
            let mut asn1_buf = [0u8; 1000];
            let c = Cert::new(&test_vectors::ASN1_INPUT1).unwrap();
            let len = c.as_asn1(&mut asn1_buf).unwrap();
            assert_eq!(&test_vectors::ASN1_OUTPUT1, &asn1_buf[..len]);
        }

        {
            let mut asn1_buf = [0u8; 1000];
            let c = Cert::new(&test_vectors::ASN1_INPUT2).unwrap();
            let len = c.as_asn1(&mut asn1_buf).unwrap();
            assert_eq!(&test_vectors::ASN1_OUTPUT2, &asn1_buf[..len]);
        }
    }

    #[test]
    fn test_verify_chain_success() {
        let noc = Cert::new(&test_vectors::NOC1_SUCCESS).unwrap();
        let icac = Cert::new(&test_vectors::ICAC1_SUCCESS).unwrap();
        let rca = Cert::new(&test_vectors::RCA1_SUCCESS).unwrap();
        let a = noc.verify_chain_start();
        a.add_cert(&icac)
            .unwrap()
            .add_cert(&rca)
            .unwrap()
            .finalise()
            .unwrap();
    }

    #[test]
    fn test_verify_chain_incomplete() {
        // The chain doesn't lead up to a self-signed certificate
        let noc = Cert::new(&test_vectors::NOC1_SUCCESS).unwrap();
        let icac = Cert::new(&test_vectors::ICAC1_SUCCESS).unwrap();
        let a = noc.verify_chain_start();
        assert_eq!(
            Err(Error::InvalidAuthKey),
            a.add_cert(&icac).unwrap().finalise()
        );
    }

    #[test]
    fn test_auth_key_chain_incorrect() {
        let noc = Cert::new(&test_vectors::NOC1_AUTH_KEY_FAIL).unwrap();
        let icac = Cert::new(&test_vectors::ICAC1_SUCCESS).unwrap();
        let a = noc.verify_chain_start();
        assert_eq!(Err(Error::InvalidAuthKey), a.add_cert(&icac).map(|_| ()));
    }

    #[test]
    fn test_cert_corrupted() {
        let noc = Cert::new(&test_vectors::NOC1_CORRUPT_CERT).unwrap();
        let icac = Cert::new(&test_vectors::ICAC1_SUCCESS).unwrap();
        let a = noc.verify_chain_start();
        assert_eq!(Err(Error::InvalidSignature), a.add_cert(&icac).map(|_| ()));
    }

    #[test]
    fn test_tlv_conversions() {
        let test_input: [&[u8]; 3] = [
            &test_vectors::NOC1_SUCCESS,
            &test_vectors::ICAC1_SUCCESS,
            &test_vectors::RCA1_SUCCESS,
        ];

        for input in test_input.iter() {
            println!("Testing next input...");
            let root = tlv::get_root_node(*input).unwrap();
            let cert = Cert::from_tlv(&root).unwrap();
            let mut buf = [0u8; 1024];
            let buf_len = buf.len();
            let mut wb = WriteBuf::new(&mut buf, buf_len);
            let mut tw = TLVWriter::new(&mut wb);
            cert.to_tlv(&mut tw, TagType::Anonymous).unwrap();
            assert_eq!(*input, wb.as_slice());
        }
    }

    mod test_vectors {
        // Group 1
        pub const NOC1_SUCCESS: [u8; 247] = [
            0x15, 0x30, 0x1, 0x1, 0x1, 0x24, 0x2, 0x1, 0x37, 0x3, 0x24, 0x13, 0x1, 0x24, 0x15, 0x1,
            0x18, 0x26, 0x4, 0x80, 0x22, 0x81, 0x27, 0x26, 0x5, 0x80, 0x25, 0x4d, 0x3a, 0x37, 0x6,
            0x26, 0x11, 0x2, 0x5c, 0xbc, 0x0, 0x24, 0x15, 0x1, 0x18, 0x24, 0x7, 0x1, 0x24, 0x8,
            0x1, 0x30, 0x9, 0x41, 0x4, 0xba, 0x22, 0x56, 0x43, 0x4f, 0x59, 0x98, 0x32, 0x8d, 0xb8,
            0xcb, 0x3f, 0x24, 0x90, 0x9a, 0x96, 0x94, 0x43, 0x46, 0x67, 0xc2, 0x11, 0xe3, 0x80,
            0x26, 0x65, 0xfc, 0x65, 0x37, 0x77, 0x3, 0x25, 0x18, 0xd8, 0xdc, 0x85, 0xfa, 0xe6,
            0x42, 0xe7, 0x55, 0xc9, 0x37, 0xcc, 0xb, 0x78, 0x84, 0x3d, 0x2f, 0xac, 0x81, 0x88,
            0x2e, 0x69, 0x0, 0xa5, 0xfc, 0xcd, 0xe0, 0xad, 0xb2, 0x69, 0xca, 0x73, 0x37, 0xa, 0x35,
            0x1, 0x28, 0x1, 0x18, 0x24, 0x2, 0x1, 0x36, 0x3, 0x4, 0x2, 0x4, 0x1, 0x18, 0x30, 0x4,
            0x14, 0x39, 0x68, 0x16, 0x1e, 0xb5, 0x56, 0x6d, 0xd3, 0xf8, 0x61, 0xf2, 0x95, 0xf3,
            0x55, 0xa0, 0xfb, 0xd2, 0x82, 0xc2, 0x29, 0x30, 0x5, 0x14, 0xce, 0x60, 0xb4, 0x28,
            0x96, 0x72, 0x27, 0x64, 0x81, 0xbc, 0x4f, 0x0, 0x78, 0xa3, 0x30, 0x48, 0xfe, 0x6e,
            0x65, 0x86, 0x18, 0x30, 0xb, 0x40, 0x2, 0x88, 0x42, 0x0, 0x6f, 0xcc, 0xe0, 0xf0, 0x6c,
            0xd9, 0xf9, 0x5e, 0xe4, 0xc2, 0xaa, 0x1f, 0x57, 0x71, 0x62, 0xdb, 0x6b, 0x4e, 0xe7,
            0x55, 0x3f, 0xc6, 0xc7, 0x9f, 0xf8, 0x30, 0xeb, 0x16, 0x6e, 0x6d, 0xc6, 0x9c, 0xb,
            0xb7, 0xe2, 0xb8, 0xe3, 0xe7, 0x57, 0x88, 0x7b, 0xda, 0xe5, 0x79, 0x39, 0x6d, 0x2c,
            0x37, 0xb2, 0x7f, 0xc3, 0x63, 0x2f, 0x7e, 0x70, 0xab, 0x5a, 0x2c, 0xf7, 0x5b, 0x18,
        ];
        pub const ICAC1_SUCCESS: [u8; 237] = [
            21, 48, 1, 1, 0, 36, 2, 1, 55, 3, 36, 20, 0, 36, 21, 1, 24, 38, 4, 128, 34, 129, 39,
            38, 5, 128, 37, 77, 58, 55, 6, 36, 19, 1, 36, 21, 1, 24, 36, 7, 1, 36, 8, 1, 48, 9, 65,
            4, 86, 25, 119, 24, 63, 212, 255, 43, 88, 61, 233, 121, 52, 102, 223, 233, 0, 251, 109,
            161, 239, 224, 204, 220, 119, 48, 192, 111, 182, 45, 255, 190, 84, 160, 149, 117, 11,
            139, 7, 188, 85, 219, 156, 182, 85, 19, 8, 184, 223, 2, 227, 64, 107, 174, 52, 245, 12,
            186, 201, 242, 191, 241, 231, 80, 55, 10, 53, 1, 41, 1, 24, 36, 2, 96, 48, 4, 20, 206,
            96, 180, 40, 150, 114, 39, 100, 129, 188, 79, 0, 120, 163, 48, 72, 254, 110, 101, 134,
            48, 5, 20, 212, 86, 147, 190, 112, 121, 244, 156, 112, 107, 7, 111, 17, 28, 109, 229,
            100, 164, 68, 116, 24, 48, 11, 64, 243, 8, 190, 128, 155, 254, 245, 21, 205, 241, 217,
            246, 204, 182, 247, 41, 81, 91, 33, 155, 230, 223, 212, 116, 33, 162, 208, 148, 100,
            89, 175, 253, 78, 212, 7, 69, 207, 140, 45, 129, 249, 64, 104, 70, 68, 43, 164, 19,
            126, 114, 138, 79, 104, 238, 20, 226, 88, 118, 105, 56, 12, 92, 31, 171, 24,
        ];
        // A single byte in the auth key id is changed in this
        pub const NOC1_AUTH_KEY_FAIL: [u8; 247] = [
            0x15, 0x30, 0x1, 0x1, 0x1, 0x24, 0x2, 0x1, 0x37, 0x3, 0x24, 0x13, 0x1, 0x24, 0x15, 0x1,
            0x18, 0x26, 0x4, 0x80, 0x22, 0x81, 0x27, 0x26, 0x5, 0x80, 0x25, 0x4d, 0x3a, 0x37, 0x6,
            0x26, 0x11, 0x2, 0x5c, 0xbc, 0x0, 0x24, 0x15, 0x1, 0x18, 0x24, 0x7, 0x1, 0x24, 0x8,
            0x1, 0x30, 0x9, 0x41, 0x4, 0xba, 0x22, 0x56, 0x43, 0x4f, 0x59, 0x98, 0x32, 0x8d, 0xb8,
            0xcb, 0x3f, 0x24, 0x90, 0x9a, 0x96, 0x94, 0x43, 0x46, 0x67, 0xc2, 0x11, 0xe3, 0x80,
            0x26, 0x65, 0xfc, 0x65, 0x37, 0x77, 0x3, 0x25, 0x18, 0xd8, 0xdc, 0x85, 0xfa, 0xe6,
            0x42, 0xe7, 0x55, 0xc9, 0x37, 0xcc, 0xb, 0x78, 0x84, 0x3d, 0x2f, 0xac, 0x81, 0x88,
            0x2e, 0x69, 0x0, 0xa5, 0xfc, 0xcd, 0xe0, 0xad, 0xb2, 0x69, 0xca, 0x73, 0x37, 0xa, 0x35,
            0x1, 0x28, 0x1, 0x18, 0x24, 0x2, 0x1, 0x36, 0x3, 0x4, 0x2, 0x4, 0x1, 0x18, 0x30, 0x4,
            0x14, 0x39, 0x68, 0x16, 0x1e, 0xb5, 0x56, 0x6d, 0xd3, 0xf8, 0x61, 0xf2, 0x95, 0xf3,
            0x55, 0xa0, 0xfb, 0xd2, 0x82, 0xc2, 0x29, 0x30, 0x5, 0x14, 0xce, 0x61, 0xb4, 0x28,
            0x96, 0x72, 0x27, 0x64, 0x81, 0xbc, 0x4f, 0x0, 0x78, 0xa3, 0x30, 0x48, 0xfe, 0x6e,
            0x65, 0x86, 0x18, 0x30, 0xb, 0x40, 0x2, 0x88, 0x42, 0x0, 0x6f, 0xcc, 0xe0, 0xf0, 0x6c,
            0xd9, 0xf9, 0x5e, 0xe4, 0xc2, 0xaa, 0x1f, 0x57, 0x71, 0x62, 0xdb, 0x6b, 0x4e, 0xe7,
            0x55, 0x3f, 0xc6, 0xc7, 0x9f, 0xf8, 0x30, 0xeb, 0x16, 0x6e, 0x6d, 0xc6, 0x9c, 0xb,
            0xb7, 0xe2, 0xb8, 0xe3, 0xe7, 0x57, 0x88, 0x7b, 0xda, 0xe5, 0x79, 0x39, 0x6d, 0x2c,
            0x37, 0xb2, 0x7f, 0xc3, 0x63, 0x2f, 0x7e, 0x70, 0xab, 0x5a, 0x2c, 0xf7, 0x5b, 0x18,
        ];
        // A single byte in the Certificate contents is changed in this
        pub const NOC1_CORRUPT_CERT: [u8; 247] = [
            0x15, 0x30, 0x1, 0x1, 0x1, 0x24, 0x2, 0x1, 0x37, 0x3, 0x24, 0x13, 0x1, 0x24, 0x15, 0x1,
            0x18, 0x26, 0x4, 0x80, 0x22, 0x81, 0x27, 0x26, 0x5, 0x80, 0x25, 0x4d, 0x3a, 0x37, 0x6,
            0x26, 0x11, 0x2, 0x5c, 0xbc, 0x0, 0x24, 0x15, 0x1, 0x18, 0x24, 0x7, 0x1, 0x24, 0x8,
            0x1, 0x30, 0x9, 0x41, 0x4, 0xba, 0x23, 0x56, 0x43, 0x4f, 0x59, 0x98, 0x32, 0x8d, 0xb8,
            0xcb, 0x3f, 0x24, 0x90, 0x9a, 0x96, 0x94, 0x43, 0x46, 0x67, 0xc2, 0x11, 0xe3, 0x80,
            0x26, 0x65, 0xfc, 0x65, 0x37, 0x77, 0x3, 0x25, 0x18, 0xd8, 0xdc, 0x85, 0xfa, 0xe6,
            0x42, 0xe7, 0x55, 0xc9, 0x37, 0xcc, 0xb, 0x78, 0x84, 0x3d, 0x2f, 0xac, 0x81, 0x88,
            0x2e, 0x69, 0x0, 0xa5, 0xfc, 0xcd, 0xe0, 0xad, 0xb2, 0x69, 0xca, 0x73, 0x37, 0xa, 0x35,
            0x1, 0x28, 0x1, 0x18, 0x24, 0x2, 0x1, 0x36, 0x3, 0x4, 0x2, 0x4, 0x1, 0x18, 0x30, 0x4,
            0x14, 0x39, 0x68, 0x16, 0x1e, 0xb5, 0x56, 0x6d, 0xd3, 0xf8, 0x61, 0xf2, 0x95, 0xf3,
            0x55, 0xa0, 0xfb, 0xd2, 0x82, 0xc2, 0x29, 0x30, 0x5, 0x14, 0xce, 0x60, 0xb4, 0x28,
            0x96, 0x72, 0x27, 0x64, 0x81, 0xbc, 0x4f, 0x0, 0x78, 0xa3, 0x30, 0x48, 0xfe, 0x6e,
            0x65, 0x86, 0x18, 0x30, 0xb, 0x40, 0x2, 0x88, 0x42, 0x0, 0x6f, 0xcc, 0xe0, 0xf0, 0x6c,
            0xd9, 0xf9, 0x5e, 0xe4, 0xc2, 0xaa, 0x1f, 0x57, 0x71, 0x62, 0xdb, 0x6b, 0x4e, 0xe7,
            0x55, 0x3f, 0xc6, 0xc7, 0x9f, 0xf8, 0x30, 0xeb, 0x16, 0x6e, 0x6d, 0xc6, 0x9c, 0xb,
            0xb7, 0xe2, 0xb8, 0xe3, 0xe7, 0x57, 0x88, 0x7b, 0xda, 0xe5, 0x79, 0x39, 0x6d, 0x2c,
            0x37, 0xb2, 0x7f, 0xc3, 0x63, 0x2f, 0x7e, 0x70, 0xab, 0x5a, 0x2c, 0xf7, 0x5b, 0x18,
        ];
        pub const RCA1_SUCCESS: [u8; 237] = [
            0x15, 0x30, 0x1, 0x1, 0x0, 0x24, 0x2, 0x1, 0x37, 0x3, 0x24, 0x14, 0x0, 0x24, 0x15, 0x1,
            0x18, 0x26, 0x4, 0x80, 0x22, 0x81, 0x27, 0x26, 0x5, 0x80, 0x25, 0x4d, 0x3a, 0x37, 0x6,
            0x24, 0x14, 0x0, 0x24, 0x15, 0x1, 0x18, 0x24, 0x7, 0x1, 0x24, 0x8, 0x1, 0x30, 0x9,
            0x41, 0x4, 0x6d, 0x70, 0x7e, 0x4b, 0x98, 0xf6, 0x2b, 0xab, 0x44, 0xd6, 0xfe, 0xa3,
            0x2e, 0x39, 0xd8, 0xc3, 0x0, 0xa0, 0xe, 0xa8, 0x6c, 0x83, 0xff, 0x69, 0xd, 0xe8, 0x42,
            0x1, 0xeb, 0xd, 0xaa, 0x68, 0x5d, 0xcb, 0x97, 0x2, 0x80, 0x1d, 0xa8, 0x50, 0x2, 0x2e,
            0x5a, 0xa2, 0x5a, 0x2e, 0x51, 0x26, 0x4, 0xd2, 0x39, 0x62, 0xcd, 0x82, 0x38, 0x63,
            0x28, 0xbf, 0x15, 0x1c, 0xa6, 0x27, 0xe0, 0xd7, 0x37, 0xa, 0x35, 0x1, 0x29, 0x1, 0x18,
            0x24, 0x2, 0x60, 0x30, 0x4, 0x14, 0xd4, 0x56, 0x93, 0xbe, 0x70, 0x79, 0xf4, 0x9c, 0x70,
            0x6b, 0x7, 0x6f, 0x11, 0x1c, 0x6d, 0xe5, 0x64, 0xa4, 0x44, 0x74, 0x30, 0x5, 0x14, 0xd4,
            0x56, 0x93, 0xbe, 0x70, 0x79, 0xf4, 0x9c, 0x70, 0x6b, 0x7, 0x6f, 0x11, 0x1c, 0x6d,
            0xe5, 0x64, 0xa4, 0x44, 0x74, 0x18, 0x30, 0xb, 0x40, 0x3, 0xd, 0x77, 0xe1, 0x9e, 0xea,
            0x9c, 0x5, 0x5c, 0xcc, 0x47, 0xe8, 0xb3, 0x18, 0x1a, 0xd1, 0x74, 0xee, 0xc6, 0x2e,
            0xa1, 0x20, 0x16, 0xbd, 0x20, 0xb4, 0x3d, 0xac, 0x24, 0xbe, 0x17, 0xf9, 0xe, 0xb7,
            0x9a, 0x98, 0xc8, 0xbc, 0x6a, 0xce, 0x99, 0x2a, 0x2e, 0x63, 0x4c, 0x76, 0x6, 0x45,
            0x93, 0xd3, 0x7c, 0x4, 0x0, 0xe4, 0xc7, 0x78, 0xe9, 0x83, 0x5b, 0xc, 0x33, 0x61, 0x5c,
            0x2e, 0x18,
        ];
        pub const ASN1_INPUT1: [u8; 237] = [
            0x15, 0x30, 0x01, 0x01, 0x00, 0x24, 0x02, 0x01, 0x37, 0x03, 0x24, 0x14, 0x00, 0x24,
            0x15, 0x03, 0x18, 0x26, 0x04, 0x80, 0x22, 0x81, 0x27, 0x26, 0x05, 0x80, 0x25, 0x4d,
            0x3a, 0x37, 0x06, 0x24, 0x13, 0x01, 0x24, 0x15, 0x03, 0x18, 0x24, 0x07, 0x01, 0x24,
            0x08, 0x01, 0x30, 0x09, 0x41, 0x04, 0x69, 0xda, 0xe9, 0x42, 0x88, 0xcf, 0x64, 0x94,
            0x2d, 0xd5, 0x0a, 0x74, 0x2d, 0x50, 0xe8, 0x5e, 0xbe, 0x15, 0x53, 0x24, 0xe5, 0xc5,
            0x6b, 0xe5, 0x7f, 0xc1, 0x41, 0x11, 0x21, 0xdd, 0x46, 0xa3, 0x0d, 0x63, 0xc3, 0xe3,
            0x90, 0x7a, 0x69, 0x64, 0xdd, 0x66, 0x78, 0x10, 0xa6, 0xc8, 0x0f, 0xfd, 0xb6, 0xf2,
            0x9b, 0x88, 0x50, 0x93, 0x77, 0x9e, 0xf7, 0xb4, 0xda, 0x94, 0x11, 0x33, 0x1e, 0xfe,
            0x37, 0x0a, 0x35, 0x01, 0x29, 0x01, 0x18, 0x24, 0x02, 0x60, 0x30, 0x04, 0x14, 0xdf,
            0xfb, 0x79, 0xf1, 0x2b, 0xbf, 0x68, 0x18, 0x59, 0x7f, 0xf7, 0xe8, 0xaf, 0x88, 0x91,
            0x1c, 0x72, 0x32, 0xf7, 0x52, 0x30, 0x05, 0x14, 0xed, 0x31, 0x5e, 0x1a, 0xb7, 0xb9,
            0x7a, 0xca, 0x04, 0x79, 0x5d, 0x82, 0x57, 0x7a, 0xd7, 0x0a, 0x75, 0xd0, 0xdb, 0x7a,
            0x18, 0x30, 0x0b, 0x40, 0xe5, 0xd4, 0xe6, 0x0e, 0x98, 0x62, 0x2f, 0xaa, 0x59, 0xe0,
            0x28, 0x59, 0xc2, 0xd4, 0xcd, 0x34, 0x85, 0x7f, 0x93, 0xbe, 0x14, 0x35, 0xa3, 0x76,
            0x8a, 0xc9, 0x2f, 0x59, 0x39, 0xa0, 0xb0, 0x75, 0xe8, 0x8e, 0x11, 0xa9, 0xc1, 0x9e,
            0xaa, 0xab, 0xa0, 0xdb, 0xb4, 0x79, 0x63, 0xfc, 0x02, 0x03, 0x27, 0x25, 0xac, 0x21,
            0x6f, 0xef, 0x27, 0xab, 0x0f, 0x90, 0x09, 0x99, 0x05, 0xa8, 0x60, 0xd8, 0x18,
        ];
        pub const ASN1_INPUT2: [u8; 247] = [
            0x15, 0x30, 0x01, 0x01, 0x01, 0x24, 0x02, 0x01, 0x37, 0x03, 0x24, 0x13, 0x01, 0x24,
            0x15, 0x03, 0x18, 0x26, 0x04, 0x80, 0x22, 0x81, 0x27, 0x26, 0x05, 0x80, 0x25, 0x4d,
            0x3a, 0x37, 0x06, 0x26, 0x11, 0x69, 0xb6, 0x01, 0x00, 0x24, 0x15, 0x03, 0x18, 0x24,
            0x07, 0x01, 0x24, 0x08, 0x01, 0x30, 0x09, 0x41, 0x04, 0x93, 0x04, 0xc6, 0xc4, 0xe1,
            0xbc, 0x9a, 0xc8, 0xf5, 0xb3, 0x7f, 0x83, 0xd6, 0x7f, 0x79, 0xc5, 0x35, 0xdc, 0x7f,
            0xac, 0x87, 0xca, 0xcd, 0x08, 0x80, 0x4a, 0x55, 0x60, 0x80, 0x09, 0xd3, 0x9b, 0x4a,
            0xc8, 0xe7, 0x7b, 0x4d, 0x5c, 0x82, 0x88, 0x24, 0xdf, 0x1c, 0xfd, 0xef, 0xb4, 0xbc,
            0xb7, 0x2f, 0x36, 0xf7, 0x2b, 0xb2, 0xcc, 0x14, 0x69, 0x63, 0xcc, 0x89, 0xd2, 0x74,
            0x3f, 0xd1, 0x98, 0x37, 0x0a, 0x35, 0x01, 0x28, 0x01, 0x18, 0x24, 0x02, 0x01, 0x36,
            0x03, 0x04, 0x02, 0x04, 0x01, 0x18, 0x30, 0x04, 0x14, 0x9c, 0xe7, 0xd9, 0xa8, 0x6b,
            0xf8, 0x71, 0xfa, 0x08, 0x10, 0xa3, 0xf2, 0x3a, 0x95, 0x30, 0xb1, 0x9e, 0xae, 0xc4,
            0x2c, 0x30, 0x05, 0x14, 0xdf, 0xfb, 0x79, 0xf1, 0x2b, 0xbf, 0x68, 0x18, 0x59, 0x7f,
            0xf7, 0xe8, 0xaf, 0x88, 0x91, 0x1c, 0x72, 0x32, 0xf7, 0x52, 0x18, 0x30, 0x0b, 0x40,
            0xcf, 0x01, 0x37, 0x65, 0xd6, 0x8a, 0xca, 0xd8, 0x33, 0x9f, 0x0f, 0x4f, 0xd5, 0xed,
            0x48, 0x42, 0x91, 0xca, 0xab, 0xf7, 0xae, 0xe1, 0x3b, 0x2b, 0xef, 0x9f, 0x43, 0x5a,
            0x96, 0xe0, 0xa5, 0x38, 0x8e, 0x39, 0xd0, 0x20, 0x8a, 0x0c, 0x92, 0x2b, 0x21, 0x7d,
            0xf5, 0x6c, 0x1d, 0x65, 0x6c, 0x0f, 0xd1, 0xe8, 0x55, 0x14, 0x5e, 0x27, 0xfd, 0xa4,
            0xac, 0xf9, 0x93, 0xdb, 0x29, 0x49, 0xaa, 0x71, 0x18,
        ];

        pub const ASN1_OUTPUT1: [u8; 388] = [
            0x30, 0x82, 0x01, 0x80, 0xa0, 0x03, 0x02, 0x01, 0x02, 0x02, 0x01, 0x00, 0x30, 0x0a,
            0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02, 0x30, 0x44, 0x31, 0x20,
            0x30, 0x1e, 0x06, 0x0a, 0x2b, 0x06, 0x01, 0x04, 0x01, 0x82, 0xa2, 0x7c, 0x01, 0x04,
            0x0c, 0x10, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30,
            0x30, 0x30, 0x30, 0x30, 0x31, 0x20, 0x30, 0x1e, 0x06, 0x0a, 0x2b, 0x06, 0x01, 0x04,
            0x01, 0x82, 0xa2, 0x7c, 0x01, 0x05, 0x0c, 0x10, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30,
            0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x33, 0x30, 0x1e, 0x17, 0x0d,
            0x32, 0x31, 0x30, 0x31, 0x30, 0x31, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x5a, 0x17,
            0x0d, 0x33, 0x30, 0x31, 0x32, 0x33, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x5a,
            0x30, 0x44, 0x31, 0x20, 0x30, 0x1e, 0x06, 0x0a, 0x2b, 0x06, 0x01, 0x04, 0x01, 0x82,
            0xa2, 0x7c, 0x01, 0x03, 0x0c, 0x10, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30,
            0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x31, 0x31, 0x20, 0x30, 0x1e, 0x06, 0x0a,
            0x2b, 0x06, 0x01, 0x04, 0x01, 0x82, 0xa2, 0x7c, 0x01, 0x05, 0x0c, 0x10, 0x30, 0x30,
            0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x33,
            0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06,
            0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00, 0x04, 0x69,
            0xda, 0xe9, 0x42, 0x88, 0xcf, 0x64, 0x94, 0x2d, 0xd5, 0x0a, 0x74, 0x2d, 0x50, 0xe8,
            0x5e, 0xbe, 0x15, 0x53, 0x24, 0xe5, 0xc5, 0x6b, 0xe5, 0x7f, 0xc1, 0x41, 0x11, 0x21,
            0xdd, 0x46, 0xa3, 0x0d, 0x63, 0xc3, 0xe3, 0x90, 0x7a, 0x69, 0x64, 0xdd, 0x66, 0x78,
            0x10, 0xa6, 0xc8, 0x0f, 0xfd, 0xb6, 0xf2, 0x9b, 0x88, 0x50, 0x93, 0x77, 0x9e, 0xf7,
            0xb4, 0xda, 0x94, 0x11, 0x33, 0x1e, 0xfe, 0xa3, 0x63, 0x30, 0x61, 0x30, 0x0f, 0x06,
            0x03, 0x55, 0x1d, 0x13, 0x01, 0x01, 0xff, 0x04, 0x05, 0x30, 0x03, 0x01, 0x01, 0xff,
            0x30, 0x0e, 0x06, 0x03, 0x55, 0x1d, 0x0f, 0x01, 0x01, 0xff, 0x04, 0x04, 0x03, 0x02,
            0x01, 0x06, 0x30, 0x1d, 0x06, 0x03, 0x55, 0x1d, 0x0e, 0x04, 0x16, 0x04, 0x14, 0xdf,
            0xfb, 0x79, 0xf1, 0x2b, 0xbf, 0x68, 0x18, 0x59, 0x7f, 0xf7, 0xe8, 0xaf, 0x88, 0x91,
            0x1c, 0x72, 0x32, 0xf7, 0x52, 0x30, 0x1f, 0x06, 0x03, 0x55, 0x1d, 0x23, 0x04, 0x18,
            0x30, 0x16, 0x80, 0x14, 0xed, 0x31, 0x5e, 0x1a, 0xb7, 0xb9, 0x7a, 0xca, 0x04, 0x79,
            0x5d, 0x82, 0x57, 0x7a, 0xd7, 0x0a, 0x75, 0xd0, 0xdb, 0x7a,
        ];
        pub const ASN1_OUTPUT2: [u8; 421] = [
            0x30, 0x82, 0x01, 0xa1, 0xa0, 0x03, 0x02, 0x01, 0x02, 0x02, 0x01, 0x01, 0x30, 0x0a,
            0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02, 0x30, 0x44, 0x31, 0x20,
            0x30, 0x1e, 0x06, 0x0a, 0x2b, 0x06, 0x01, 0x04, 0x01, 0x82, 0xa2, 0x7c, 0x01, 0x03,
            0x0c, 0x10, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30,
            0x30, 0x30, 0x30, 0x31, 0x31, 0x20, 0x30, 0x1e, 0x06, 0x0a, 0x2b, 0x06, 0x01, 0x04,
            0x01, 0x82, 0xa2, 0x7c, 0x01, 0x05, 0x0c, 0x10, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30,
            0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x33, 0x30, 0x1e, 0x17, 0x0d,
            0x32, 0x31, 0x30, 0x31, 0x30, 0x31, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x5a, 0x17,
            0x0d, 0x33, 0x30, 0x31, 0x32, 0x33, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x5a,
            0x30, 0x44, 0x31, 0x20, 0x30, 0x1e, 0x06, 0x0a, 0x2b, 0x06, 0x01, 0x04, 0x01, 0x82,
            0xa2, 0x7c, 0x01, 0x01, 0x0c, 0x10, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30,
            0x30, 0x30, 0x30, 0x31, 0x42, 0x36, 0x36, 0x39, 0x31, 0x20, 0x30, 0x1e, 0x06, 0x0a,
            0x2b, 0x06, 0x01, 0x04, 0x01, 0x82, 0xa2, 0x7c, 0x01, 0x05, 0x0c, 0x10, 0x30, 0x30,
            0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x33,
            0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06,
            0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00, 0x04, 0x93,
            0x04, 0xc6, 0xc4, 0xe1, 0xbc, 0x9a, 0xc8, 0xf5, 0xb3, 0x7f, 0x83, 0xd6, 0x7f, 0x79,
            0xc5, 0x35, 0xdc, 0x7f, 0xac, 0x87, 0xca, 0xcd, 0x08, 0x80, 0x4a, 0x55, 0x60, 0x80,
            0x09, 0xd3, 0x9b, 0x4a, 0xc8, 0xe7, 0x7b, 0x4d, 0x5c, 0x82, 0x88, 0x24, 0xdf, 0x1c,
            0xfd, 0xef, 0xb4, 0xbc, 0xb7, 0x2f, 0x36, 0xf7, 0x2b, 0xb2, 0xcc, 0x14, 0x69, 0x63,
            0xcc, 0x89, 0xd2, 0x74, 0x3f, 0xd1, 0x98, 0xa3, 0x81, 0x83, 0x30, 0x81, 0x80, 0x30,
            0x0c, 0x06, 0x03, 0x55, 0x1d, 0x13, 0x01, 0x01, 0xff, 0x04, 0x02, 0x30, 0x00, 0x30,
            0x0e, 0x06, 0x03, 0x55, 0x1d, 0x0f, 0x01, 0x01, 0xff, 0x04, 0x04, 0x03, 0x02, 0x07,
            0x80, 0x30, 0x20, 0x06, 0x03, 0x55, 0x1d, 0x25, 0x01, 0x01, 0xff, 0x04, 0x16, 0x30,
            0x14, 0x06, 0x08, 0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x02, 0x06, 0x08, 0x2b,
            0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x01, 0x30, 0x1d, 0x06, 0x03, 0x55, 0x1d, 0x0e,
            0x04, 0x16, 0x04, 0x14, 0x9c, 0xe7, 0xd9, 0xa8, 0x6b, 0xf8, 0x71, 0xfa, 0x08, 0x10,
            0xa3, 0xf2, 0x3a, 0x95, 0x30, 0xb1, 0x9e, 0xae, 0xc4, 0x2c, 0x30, 0x1f, 0x06, 0x03,
            0x55, 0x1d, 0x23, 0x04, 0x18, 0x30, 0x16, 0x80, 0x14, 0xdf, 0xfb, 0x79, 0xf1, 0x2b,
            0xbf, 0x68, 0x18, 0x59, 0x7f, 0xf7, 0xe8, 0xaf, 0x88, 0x91, 0x1c, 0x72, 0x32, 0xf7,
            0x52,
        ];
    }
}
