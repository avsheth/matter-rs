#[derive(Debug, PartialEq, Copy, Clone)]
pub enum ExchangeRole {
    Initiator = 0,
    Responder = 1,
}

#[derive(Debug)]
pub struct Exchange {
    id: u16,
    role: ExchangeRole,
    pending_ack: Option<u32>,
}

impl Exchange {
    pub fn new(id: u16, role: ExchangeRole) -> Exchange {
        Exchange{id, role, pending_ack: None}
    }

    pub fn is_match(&self, id: u16, role: ExchangeRole) -> bool {
        self.id == id && self.role == role
    }

    pub fn ack_pending(&mut self, ack_ctr: u32) {
        self.pending_ack = Some(ack_ctr);
    }
}
