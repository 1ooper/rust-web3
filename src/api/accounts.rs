//! Partial implementation of the `Accounts` namespace.

use crate::{
    api::Namespace,
    signing,
    types::{AccessList, H256, U64},
    Transport,
};

/// `Accounts` namespace
#[derive(Debug, Clone)]
pub struct Accounts<T> {
    transport: T,
}

impl<T: Transport> Namespace<T> for Accounts<T> {
    fn new(transport: T) -> Self
    where
        Self: Sized,
    {
        Accounts { transport }
    }

    fn transport(&self) -> &T {
        &self.transport
    }
}

impl<T: Transport> Accounts<T> {
    /// Hash a message according to EIP-191.
    ///
    /// The data is a UTF-8 encoded string and will enveloped as follows:
    /// `"\x19Ethereum Signed Message:\n" + message.length + message` and hashed
    /// using keccak256.
    pub fn hash_message<S>(&self, message: S) -> H256
    where
        S: AsRef<[u8]>,
    {
        let message = message.as_ref();

        let mut eth_message = format!("\x19Ethereum Signed Message:\n{}", message.len()).into_bytes();
        eth_message.extend_from_slice(message);

        signing::keccak256(&eth_message).into()
    }
}

#[cfg(feature = "signing")]
mod accounts_signing {
    use super::*;
    use crate::{
        api::Web3,
        error,
        signing::Signature,
        types::{
            Address, Bytes, Recovery, RecoveryMessage, SignedData, SignedTransaction, TransactionParameters, U256,
        },
    };
    use rlp::RlpStream;
    use std::convert::TryInto;

    const LEGACY_TX_ID: u64 = 0;
    const ACCESSLISTS_TX_ID: u64 = 1;
    const EIP1559_TX_ID: u64 = 2;

    impl<T: Transport> Accounts<T> {
        /// Gets the parent `web3` namespace
        fn web3(&self) -> Web3<T> {
            Web3::new(self.transport.clone())
        }

        /// Signs an Ethereum transaction with a given private key.
        ///
        /// Transaction signing can perform RPC requests in order to fill missing
        /// parameters required for signing `nonce`, `gas_price` and `chain_id`. Note
        /// that if all transaction parameters were provided, this future will resolve
        /// immediately.
        pub async fn sign_transaction<K: signing::Key>(
            &self,
            tx: TransactionParameters,
            key: K,
        ) -> error::Result<SignedTransaction> {
            macro_rules! maybe {
                ($o: expr, $f: expr) => {
                    async {
                        match $o {
                            Some(value) => Ok(value),
                            None => $f.await,
                        }
                    }
                };
            }
            let from = key.address();

            let gas_price = match tx.transaction_type {
                Some(tx_type) if tx_type == U64::from(EIP1559_TX_ID) && tx.max_fee_per_gas.is_some() => {
                    tx.max_fee_per_gas
                }
                _ => tx.gas_price,
            };

            let (nonce, gas_price, chain_id) = futures::future::try_join3(
                maybe!(tx.nonce, self.web3().eth().transaction_count(from, None)),
                maybe!(gas_price, self.web3().eth().gas_price()),
                maybe!(tx.chain_id.map(U256::from), self.web3().eth().chain_id()),
            )
            .await?;
            let chain_id = chain_id.as_u64();

            let max_priority_fee_per_gas = match tx.transaction_type {
                Some(tx_type) if tx_type == U64::from(EIP1559_TX_ID) => {
                    tx.max_priority_fee_per_gas.unwrap_or(gas_price)
                }
                _ => gas_price,
            };

            let tx = Transaction {
                to: tx.to,
                nonce,
                gas: tx.gas,
                gas_price,
                value: tx.value,
                data: tx.data.0,
                transaction_type: tx.transaction_type,
                access_list: tx.access_list.unwrap_or_default(),
                max_priority_fee_per_gas,
            };

            let signed = tx.sign(key, chain_id);
            Ok(signed)
        }

        ///fill tx
        pub async fn fill_transaction (
            &self,
            tx: TransactionParameters,
            from: &Address,
        ) -> error::Result<TxWithID> {
            macro_rules! maybe {
                ($o: expr, $f: expr) => {
                    async {
                        match $o {
                            Some(value) => Ok(value),
                            None => $f.await,
                        }
                    }
                };
            }

            let gas_price = match tx.transaction_type {
                Some(tx_type) if tx_type == U64::from(EIP1559_TX_ID) && tx.max_fee_per_gas.is_some() => {
                    tx.max_fee_per_gas
                }
                _ => tx.gas_price,
            };

            let (nonce, gas_price, chain_id) = futures::future::try_join3(
                maybe!(tx.nonce, self.web3().eth().transaction_count(from.clone(), None)),
                maybe!(gas_price, self.web3().eth().gas_price()),
                maybe!(tx.chain_id.map(U256::from), self.web3().eth().chain_id()),
            )
            .await?;
            let chain_id = chain_id.as_u64();

            let max_priority_fee_per_gas = match tx.transaction_type {
                Some(tx_type) if tx_type == U64::from(EIP1559_TX_ID) => {
                    tx.max_priority_fee_per_gas.unwrap_or(gas_price)
                }
                _ => gas_price,
            };

            let tx = Transaction {
                to: tx.to,
                nonce,
                gas: tx.gas,
                gas_price,
                value: tx.value,
                data: tx.data.0,
                transaction_type: tx.transaction_type,
                access_list: tx.access_list.unwrap_or_default(),
                max_priority_fee_per_gas,
            };

            let tx_ci = TxWithID {
                tx:tx,
                chain_id:chain_id,
            };
            Ok(tx_ci)
        }


        /// Sign arbitrary string data.
        ///
        /// The data is UTF-8 encoded and enveloped the same way as with
        /// `hash_message`. The returned signed data's signature is in 'Electrum'
        /// notation, that is the recovery value `v` is either `27` or `28` (as
        /// opposed to the standard notation where `v` is either `0` or `1`). This
        /// is important to consider when using this signature with other crates.
        pub fn sign<S>(&self, message: S, key: impl signing::Key) -> SignedData
        where
            S: AsRef<[u8]>,
        {
            let message = message.as_ref();
            let message_hash = self.hash_message(message);

            let signature = key
                .sign(&message_hash.as_bytes(), None)
                .expect("hash is non-zero 32-bytes; qed");
            let v = signature
                .v
                .try_into()
                .expect("signature recovery in electrum notation always fits in a u8");

            let signature_bytes = Bytes({
                let mut bytes = Vec::with_capacity(65);
                bytes.extend_from_slice(signature.r.as_bytes());
                bytes.extend_from_slice(signature.s.as_bytes());
                bytes.push(v);
                bytes
            });

            // We perform this allocation only after all previous fallible actions have completed successfully.
            let message = message.to_owned();

            SignedData {
                message,
                message_hash,
                v,
                r: signature.r,
                s: signature.s,
                signature: signature_bytes,
            }
        }

        /// Recovers the Ethereum address which was used to sign the given data.
        ///
        /// Recovery signature data uses 'Electrum' notation, this means the `v`
        /// value is expected to be either `27` or `28`.
        pub fn recover<R>(&self, recovery: R) -> error::Result<Address>
        where
            R: Into<Recovery>,
        {
            let recovery = recovery.into();
            let message_hash = match recovery.message {
                RecoveryMessage::Data(ref message) => self.hash_message(message),
                RecoveryMessage::Hash(hash) => hash,
            };
            let (signature, recovery_id) = recovery
                .as_signature()
                .ok_or_else(|| error::Error::Recovery(signing::RecoveryError::InvalidSignature))?;
            let address = signing::recover(message_hash.as_bytes(), &signature, recovery_id)?;
            Ok(address)
        }
    }
    /// A transaction used for RLP encoding, hashing and signing.
    #[derive(Debug, Clone)]
    pub struct Transaction {
        pub to: Option<Address>,
        pub nonce: U256,
        pub gas: U256,
        pub gas_price: U256,
        pub value: U256,
        pub data: Vec<u8>,
        pub transaction_type: Option<U64>,
        pub access_list: AccessList,
        pub max_priority_fee_per_gas: U256,
    }

    impl Transaction {
        fn rlp_append_legacy(&self, stream: &mut RlpStream) {
            stream.append(&self.nonce);
            stream.append(&self.gas_price);
            stream.append(&self.gas);
            if let Some(to) = self.to {
                stream.append(&to);
            } else {
                stream.append(&"");
            }
            stream.append(&self.value);
            stream.append(&self.data);
        }

        fn encode_legacy(&self, chain_id: u64, signature: Option<&Signature>) -> RlpStream {
            let mut stream = RlpStream::new();
            stream.begin_list(9);

            self.rlp_append_legacy(&mut stream);

            if let Some(signature) = signature {
                self.rlp_append_signature(&mut stream, signature);
            } else {
                stream.append(&chain_id);
                stream.append(&0u8);
                stream.append(&0u8);
            }

            stream
        }

        fn encode_access_list_payload(&self, chain_id: u64, signature: Option<&Signature>) -> RlpStream {
            let mut stream = RlpStream::new();

            let list_size = if signature.is_some() { 11 } else { 8 };
            stream.begin_list(list_size);

            // append chain_id. from EIP-2930: chainId is defined to be an integer of arbitrary size.
            stream.append(&chain_id);

            self.rlp_append_legacy(&mut stream);
            self.rlp_append_access_list(&mut stream);

            if let Some(signature) = signature {
                self.rlp_append_signature(&mut stream, signature);
            }

            stream
        }

        fn encode_eip1559_payload(&self, chain_id: u64, signature: Option<&Signature>) -> RlpStream {
            let mut stream = RlpStream::new();

            let list_size = if signature.is_some() { 12 } else { 9 };
            stream.begin_list(list_size);

            // append chain_id. from EIP-2930: chainId is defined to be an integer of arbitrary size.
            stream.append(&chain_id);

            stream.append(&self.nonce);
            stream.append(&self.max_priority_fee_per_gas);
            stream.append(&self.gas_price);
            stream.append(&self.gas);
            if let Some(to) = self.to {
                stream.append(&to);
            } else {
                stream.append(&"");
            }
            stream.append(&self.value);
            stream.append(&self.data);

            self.rlp_append_access_list(&mut stream);

            if let Some(signature) = signature {
                self.rlp_append_signature(&mut stream, signature);
            }

            stream
        }

        fn rlp_append_signature(&self, stream: &mut RlpStream, signature: &Signature) -> () {
            stream.append(&signature.v);
            stream.append(&U256::from_big_endian(signature.r.as_bytes()));
            stream.append(&U256::from_big_endian(signature.s.as_bytes()));
        }

        fn rlp_append_access_list(&self, stream: &mut RlpStream) -> () {
            stream.begin_list(self.access_list.len());
            for access in self.access_list.iter() {
                stream.begin_list(2);
                stream.append(&access.address);
                stream.begin_list(access.storage_keys.len());
                for storage_key in access.storage_keys.iter() {
                    stream.append(storage_key);
                }
            }
        }

        fn encode(&self, chain_id: u64, signature: Option<&Signature>) -> Vec<u8> {
            match self.transaction_type.map(|t| t.as_u64()) {
                Some(LEGACY_TX_ID) | None => {
                    let stream = self.encode_legacy(chain_id, signature);
                    stream.out().to_vec()
                }

                Some(ACCESSLISTS_TX_ID) => {
                    let tx_id: u8 = ACCESSLISTS_TX_ID as u8;
                    let stream = self.encode_access_list_payload(chain_id, signature);
                    [&[tx_id], stream.as_raw()].concat()
                }

                Some(EIP1559_TX_ID) => {
                    let tx_id: u8 = EIP1559_TX_ID as u8;
                    let stream = self.encode_eip1559_payload(chain_id, signature);
                    [&[tx_id], stream.as_raw()].concat()
                }

                _ => {
                    panic!("Unsupported transaction type");
                }
            }
        }

        /// Sign and return a raw signed transaction.
        pub fn sign(self, sign: impl signing::Key, chain_id: u64) -> SignedTransaction {
            let adjust_v_value = match self.transaction_type.map(|t| t.as_u64()) {
                Some(LEGACY_TX_ID) | None => true,
                _ => false,
            };

            let encoded = self.encode(chain_id, None);

            let hash = signing::keccak256(encoded.as_ref());

            let signature = if adjust_v_value {
                sign.sign(&hash, Some(chain_id))
                    .expect("hash is non-zero 32-bytes; qed")
            } else {
                sign.sign_message(&hash).expect("hash is non-zero 32-bytes; qed")
            };

            let signed = self.encode(chain_id, Some(&signature));
            let transaction_hash = signing::keccak256(signed.as_ref()).into();

            SignedTransaction {
                message_hash: hash.into(),
                v: signature.v,
                r: signature.r,
                s: signature.s,
                raw_transaction: signed.into(),
                transaction_hash,
            }
        }

        pub fn tx_raw_transaction(self, chain_id: u64, sig: &Signature) -> Bytes {
            self.encode(chain_id, Some(sig)).into()
        }

        pub fn tx_hash(self, chain_id: u64) -> [u8;32] {
            let encoded = self.encode(chain_id, None);
            signing::keccak256(encoded.as_ref())
        }
    }

    pub struct TxWithID {
        pub tx: Transaction,
        pub chain_id: u64,
    }
}


#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::{
        signing::{SecretKey, SecretKeyRef},
        transports::test::TestTransport,
        types::{Address, Recovery, SignedTransaction, TransactionParameters, U256},
    };
    use accounts_signing::*;
    use hex_literal::hex;
    use serde_json::json;

    #[test]
    fn accounts_sign_transaction() {
        // retrieved test vector from:
        // https://web3js.readthedocs.io/en/v1.2.0/web3-eth-accounts.html#eth-accounts-signtransaction

        let tx = TransactionParameters {
            to: Some(hex!("F0109fC8DF283027b6285cc889F5aA624EaC1F55").into()),
            value: 1_000_000_000.into(),
            gas: 2_000_000.into(),
            ..Default::default()
        };
        let key = SecretKey::from_slice(&hex!(
            "4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318"
        ))
        .unwrap();
        let nonce = U256::zero();
        let gas_price = U256::from(21_000_000_000u128);
        let chain_id = "0x1";
        let from: Address = signing::secret_key_address(&key);

        let mut transport = TestTransport::default();
        transport.add_response(json!(nonce));
        transport.add_response(json!(gas_price));
        transport.add_response(json!(chain_id));

        let signed = {
            let accounts = Accounts::new(&transport);
            futures::executor::block_on(accounts.sign_transaction(tx, &key))
        };

        transport.assert_request(
            "eth_getTransactionCount",
            &[json!(from).to_string(), json!("latest").to_string()],
        );
        transport.assert_request("eth_gasPrice", &[]);
        transport.assert_request("eth_chainId", &[]);
        transport.assert_no_more_requests();

        let expected = SignedTransaction {
            message_hash: hex!("88cfbd7e51c7a40540b233cf68b62ad1df3e92462f1c6018d6d67eae0f3b08f5").into(),
            v: 0x25,
            r: hex!("c9cf86333bcb065d140032ecaab5d9281bde80f21b9687b3e94161de42d51895").into(),
            s: hex!("727a108a0b8d101465414033c3f705a9c7b826e596766046ee1183dbc8aeaa68").into(),
            raw_transaction: hex!("f869808504e3b29200831e848094f0109fc8df283027b6285cc889f5aa624eac1f55843b9aca008025a0c9cf86333bcb065d140032ecaab5d9281bde80f21b9687b3e94161de42d51895a0727a108a0b8d101465414033c3f705a9c7b826e596766046ee1183dbc8aeaa68").into(),
            transaction_hash: hex!("de8db924885b0803d2edc335f745b2b8750c8848744905684c20b987443a9593").into(),
        };

        assert_eq!(signed, Ok(expected));
    }

    #[test]
    fn accounts_sign_transaction_with_all_parameters() {
        let key = SecretKey::from_slice(&hex!(
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
        ))
        .unwrap();

        let accounts = Accounts::new(TestTransport::default());
        futures::executor::block_on(accounts.sign_transaction(
            TransactionParameters {
                nonce: Some(0.into()),
                gas_price: Some(1.into()),
                chain_id: Some(42),
                ..Default::default()
            },
            &key,
        ))
        .unwrap();

        // sign_transaction makes no requests when all parameters are specified
        accounts.transport().assert_no_more_requests();
    }

    #[test]
    fn accounts_hash_message() {
        // test vector taken from:
        // https://web3js.readthedocs.io/en/v1.2.2/web3-eth-accounts.html#hashmessage

        let accounts = Accounts::new(TestTransport::default());
        let hash = accounts.hash_message("Hello World");

        assert_eq!(
            hash,
            hex!("a1de988600a42c4b4ab089b619297c17d53cffae5d5120d82d8a92d0bb3b78f2").into()
        );

        // this method does not actually make any requests.
        accounts.transport().assert_no_more_requests();
    }

    #[test]
    fn accounts_sign() {
        // test vector taken from:
        // https://web3js.readthedocs.io/en/v1.2.2/web3-eth-accounts.html#sign

        let accounts = Accounts::new(TestTransport::default());

        let key = SecretKey::from_slice(&hex!(
            "4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318"
        ))
        .unwrap();
        let signed = accounts.sign("Some data", SecretKeyRef::new(&key));

        assert_eq!(
            signed.message_hash,
            hex!("1da44b586eb0729ff70a73c326926f6ed5a25f5b056e7f47fbc6e58d86871655").into()
        );
        assert_eq!(
            signed.signature.0,
            hex!("b91467e570a6466aa9e9876cbcd013baba02900b8979d43fe208a4a4f339f5fd6007e74cd82e037b800186422fc2da167c747ef045e5d18a5f5d4300f8e1a0291c")
        );

        // this method does not actually make any requests.
        accounts.transport().assert_no_more_requests();
    }

    #[test]
    fn accounts_recover() {
        // test vector taken from:
        // https://web3js.readthedocs.io/en/v1.2.2/web3-eth-accounts.html#recover

        let accounts = Accounts::new(TestTransport::default());

        let v = 0x1cu64;
        let r = hex!("b91467e570a6466aa9e9876cbcd013baba02900b8979d43fe208a4a4f339f5fd").into();
        let s = hex!("6007e74cd82e037b800186422fc2da167c747ef045e5d18a5f5d4300f8e1a029").into();

        let recovery = Recovery::new("Some data", v, r, s);
        assert_eq!(
            accounts.recover(recovery).unwrap(),
            hex!("2c7536E3605D9C16a7a3D7b1898e529396a65c23").into()
        );

        // this method does not actually make any requests.
        accounts.transport().assert_no_more_requests();
    }

    #[test]
    fn accounts_recover_signed() {
        let key = SecretKey::from_slice(&hex!(
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
        ))
        .unwrap();
        let address: Address = signing::secret_key_address(&key);

        let accounts = Accounts::new(TestTransport::default());

        let signed = accounts.sign("rust-web3 rocks!", &key);
        let recovered = accounts.recover(&signed).unwrap();
        assert_eq!(recovered, address);

        let signed = futures::executor::block_on(accounts.sign_transaction(
            TransactionParameters {
                nonce: Some(0.into()),
                gas_price: Some(1.into()),
                chain_id: Some(42),
                ..Default::default()
            },
            &key,
        ))
        .unwrap();
        let recovered = accounts.recover(&signed).unwrap();
        assert_eq!(recovered, address);

        // these methods make no requests
        accounts.transport().assert_no_more_requests();
    }

    #[test]
    fn sign_transaction_data() {
        // retrieved test vector from:
        // https://web3js.readthedocs.io/en/v1.2.2/web3-eth-accounts.html#eth-accounts-signtransaction

        let tx = Transaction {
            nonce: 0.into(),
            gas: 2_000_000.into(),
            gas_price: 234_567_897_654_321u64.into(),
            to: Some(hex!("F0109fC8DF283027b6285cc889F5aA624EaC1F55").into()),
            value: 1_000_000_000.into(),
            data: Vec::new(),
            transaction_type: None,
            access_list: vec![],
            max_priority_fee_per_gas: 0.into(),
        };
        let skey = SecretKey::from_slice(&hex!(
            "4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318"
        ))
        .unwrap();
        let key = SecretKeyRef::new(&skey);

        let signed = tx.sign(key, 1);

        let expected = SignedTransaction {
            message_hash: hex!("6893a6ee8df79b0f5d64a180cd1ef35d030f3e296a5361cf04d02ce720d32ec5").into(),
            v: 0x25,
            r: hex!("09ebb6ca057a0535d6186462bc0b465b561c94a295bdb0621fc19208ab149a9c").into(),
            s: hex!("440ffd775ce91a833ab410777204d5341a6f9fa91216a6f3ee2c051fea6a0428").into(),
            raw_transaction: hex!("f86a8086d55698372431831e848094f0109fc8df283027b6285cc889f5aa624eac1f55843b9aca008025a009ebb6ca057a0535d6186462bc0b465b561c94a295bdb0621fc19208ab149a9ca0440ffd775ce91a833ab410777204d5341a6f9fa91216a6f3ee2c051fea6a0428").into(),
            transaction_hash: hex!("d8f64a42b57be0d565f385378db2f6bf324ce14a594afc05de90436e9ce01f60").into(),
        };

        assert_eq!(signed, expected);
    }
}
