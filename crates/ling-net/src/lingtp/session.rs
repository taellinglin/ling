//! Session key schedule: six independent 32-byte keys, each an HKDF-SHA3
//! output of the handshake's hybrid shared secret under a distinct label,
//! salted with `client_random || server_random`. Independent keys per
//! purpose (and per direction, for the record/reactor keys) means a break
//! in one never hands over another.

pub struct SessionKeys {
    pub client_finished_key: [u8; 32],
    pub server_finished_key: [u8; 32],
    pub c2s_key: [u8; 32],
    pub s2c_key: [u8; 32],
    pub c2s_reactor_seed: [u8; 32],
    pub s2c_reactor_seed: [u8; 32],
}

pub fn derive_session_keys(
    shared_secret: &[u8; 32],
    client_random: &[u8; 32],
    server_random: &[u8; 32],
) -> SessionKeys {
    let salt = [client_random.as_slice(), server_random.as_slice()].concat();
    let derive = |label: &[u8]| -> [u8; 32] {
        let out = ling_crypto::hkdf_sha3(shared_secret, &salt, label, 32)
            .expect("hkdf_sha3: fixed 32-byte output never fails");
        let mut key = [0u8; 32];
        key.copy_from_slice(&out);
        key
    };
    SessionKeys {
        client_finished_key: derive(b"lingtp-net-v1 client-finished"),
        server_finished_key: derive(b"lingtp-net-v1 server-finished"),
        c2s_key: derive(b"lingtp-net-v1 client-to-server"),
        s2c_key: derive(b"lingtp-net-v1 server-to-client"),
        c2s_reactor_seed: derive(b"lingtp-net-v1 c2s-dice42"),
        s2c_reactor_seed: derive(b"lingtp-net-v1 s2c-dice42"),
    }
}
