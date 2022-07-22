use crate::{
    error::Error,
    interaction_model::core::OpCode,
    tlv::{get_root_node_struct, FromTLV, TLVWriter, TagType},
    transport::{packet::Packet, proto_demux::ResponseRequired},
};

use super::{
    messages::msg::{self, ReadReq},
    InteractionModel, Transaction,
};

impl InteractionModel {
    pub fn handle_read_req(
        &mut self,
        trans: &mut Transaction,
        rx_buf: &[u8],
        proto_tx: &mut Packet,
    ) -> Result<ResponseRequired, Error> {
        proto_tx.set_proto_opcode(OpCode::ReportData as u8);

        let mut tw = TLVWriter::new(proto_tx.get_writebuf()?);
        let root = get_root_node_struct(rx_buf)?;
        let read_req = ReadReq::from_tlv(&root)?;

        tw.start_struct(TagType::Anonymous)?;
        self.consumer.consume_read_attr(&read_req, trans, &mut tw)?;
        // Supress response always true for read interaction
        tw.bool(
            TagType::Context(msg::ReportDataTag::SupressResponse as u8),
            true,
        )?;
        tw.end_container()?;

        trans.complete();
        Ok(ResponseRequired::Yes)
    }
}
