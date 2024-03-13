// Copyright 2023-2024, Offchain Labs, Inc.
// For license information, see https://github.com/OffchainLabs/nitro/blob/master/LICENSE

use crate::{
    evm::{
        api::{DataReader, EvmApi, EvmApiMethod, EvmApiStatus},
        storage::{StorageCache, StorageWord},
        user::UserOutcomeKind,
    },
    format::Utf8OrHex,
    pricing::EVM_API_INK,
    Bytes20, Bytes32,
};
use eyre::{bail, eyre, Result};
use std::collections::hash_map::Entry;

pub trait RequestHandler<D: DataReader>: Send + 'static {
    fn handle_request(&mut self, req_type: EvmApiMethod, req_data: &[u8]) -> (Vec<u8>, D, u64);
}

pub struct EvmApiRequestor<D: DataReader, H: RequestHandler<D>> {
    handler: H,
    last_code: Option<(Bytes20, D)>,
    last_return_data: Option<D>,
    storage_cache: StorageCache,
}

impl<D: DataReader, H: RequestHandler<D>> EvmApiRequestor<D, H> {
    pub fn new(handler: H) -> Self {
        Self {
            handler,
            last_code: None,
            last_return_data: None,
            storage_cache: StorageCache::default(),
        }
    }

    fn handle_request(&mut self, req_type: EvmApiMethod, req_data: &[u8]) -> (Vec<u8>, D, u64) {
        self.handler.handle_request(req_type, req_data)
    }

    /// Call out to a contract.
    fn call_request(
        &mut self,
        call_type: EvmApiMethod,
        contract: Bytes20,
        input: &[u8],
        gas: u64,
        value: Bytes32,
    ) -> (u32, u64, UserOutcomeKind) {
        let mut request = Vec::with_capacity(20 + 32 + 8 + input.len());
        request.extend(contract);
        request.extend(value);
        request.extend(gas.to_be_bytes());
        request.extend(input);

        let (res, data, cost) = self.handle_request(call_type, &request);
        let status: UserOutcomeKind = res[0].try_into().expect("unknown outcome");
        let data_len = data.slice().len() as u32;
        self.last_return_data = Some(data);
        (data_len, cost, status)
    }

    pub fn request_handler(&mut self) -> &mut H {
        &mut self.handler
    }

    fn create_request(
        &mut self,
        create_type: EvmApiMethod,
        code: Vec<u8>,
        endowment: Bytes32,
        salt: Option<Bytes32>,
        gas: u64,
    ) -> (Result<Bytes20>, u32, u64) {
        let mut request = Vec::with_capacity(8 + 2 * 32 + code.len());
        request.extend(gas.to_be_bytes());
        request.extend(endowment);
        if let Some(salt) = salt {
            request.extend(salt);
        }
        request.extend(code);

        let (mut res, data, cost) = self.handle_request(create_type, &request);
        if res.len() != 21 || res[0] == 0 {
            if !res.is_empty() {
                res.remove(0);
            }
            let err_string = String::from_utf8(res).unwrap_or("create_response_malformed".into());
            return (Err(eyre!(err_string)), 0, cost);
        }
        res.remove(0);
        let address = res.try_into().unwrap();
        let data_len = data.slice().len() as u32;
        self.last_return_data = Some(data);
        (Ok(address), data_len, cost)
    }
}

impl<D: DataReader, H: RequestHandler<D>> EvmApi<D> for EvmApiRequestor<D, H> {
    fn get_bytes32(&mut self, key: Bytes32) -> (Bytes32, u64) {
        let cache = &mut self.storage_cache;
        let mut cost = cache.read_gas();

        let value = cache.entry(key).or_insert_with(|| {
            let (res, _, gas) = self
                .handler
                .handle_request(EvmApiMethod::GetBytes32, key.as_slice());
            cost = cost.saturating_add(gas).saturating_add(EVM_API_INK);
            StorageWord::known(res.try_into().unwrap())
        });
        (value.value, cost)
    }

    fn cache_bytes32(&mut self, key: Bytes32, value: Bytes32) -> u64 {
        match self.storage_cache.entry(key) {
            Entry::Occupied(mut key) => key.get_mut().value = value,
            Entry::Vacant(slot) => drop(slot.insert(StorageWord::unknown(value))),
        };
        self.storage_cache.write_gas()
    }

    fn flush_storage_cache(&mut self, clear: bool, gas_left: u64) -> Result<u64> {
        let mut data = Vec::with_capacity(64 * self.storage_cache.len() + 8);
        data.extend(gas_left.to_be_bytes());

        for (key, value) in &mut self.storage_cache.slots {
            if value.dirty() {
                data.extend(*key);
                data.extend(*value.value);
                value.known = Some(value.value);
            }
        }
        if clear {
            self.storage_cache.clear();
        }

        let (res, _, cost) = self.handle_request(EvmApiMethod::SetTrieSlots, &data);
        if res[0] != EvmApiStatus::Success.into() {
            bail!("{}", String::from_utf8_or_hex(res));
        }
        Ok(cost)
    }

    fn contract_call(
        &mut self,
        contract: Bytes20,
        input: &[u8],
        gas: u64,
        value: Bytes32,
    ) -> (u32, u64, UserOutcomeKind) {
        self.call_request(EvmApiMethod::ContractCall, contract, input, gas, value)
    }

    fn delegate_call(
        &mut self,
        contract: Bytes20,
        input: &[u8],
        gas: u64,
    ) -> (u32, u64, UserOutcomeKind) {
        self.call_request(
            EvmApiMethod::DelegateCall,
            contract,
            input,
            gas,
            Bytes32::default(),
        )
    }

    fn static_call(
        &mut self,
        contract: Bytes20,
        input: &[u8],
        gas: u64,
    ) -> (u32, u64, UserOutcomeKind) {
        self.call_request(
            EvmApiMethod::StaticCall,
            contract,
            input,
            gas,
            Bytes32::default(),
        )
    }

    fn create1(
        &mut self,
        code: Vec<u8>,
        endowment: Bytes32,
        gas: u64,
    ) -> (Result<Bytes20>, u32, u64) {
        self.create_request(EvmApiMethod::Create1, code, endowment, None, gas)
    }

    fn create2(
        &mut self,
        code: Vec<u8>,
        endowment: Bytes32,
        salt: Bytes32,
        gas: u64,
    ) -> (Result<Bytes20>, u32, u64) {
        self.create_request(EvmApiMethod::Create2, code, endowment, Some(salt), gas)
    }

    fn get_return_data(&self) -> D {
        self.last_return_data.clone().expect("missing return data")
    }

    fn emit_log(&mut self, data: Vec<u8>, topics: u32) -> Result<()> {
        // TODO: remove copy
        let mut request = Vec::with_capacity(4 + data.len());
        request.extend(topics.to_be_bytes());
        request.extend(data);

        let (res, _, _) = self.handle_request(EvmApiMethod::EmitLog, &request);
        if !res.is_empty() {
            bail!(String::from_utf8(res).unwrap_or("malformed emit-log response".into()))
        }
        Ok(())
    }

    fn account_balance(&mut self, address: Bytes20) -> (Bytes32, u64) {
        let (res, _, cost) = self.handle_request(EvmApiMethod::AccountBalance, address.as_slice());
        (res.try_into().unwrap(), cost)
    }

    fn account_code(&mut self, address: Bytes20, gas_left: u64) -> (D, u64) {
        if let Some((stored_address, data)) = self.last_code.as_ref() {
            if address == *stored_address {
                return (data.clone(), 0);
            }
        }
        let mut req = Vec::with_capacity(20 + 8);
        req.extend(address);
        req.extend(gas_left.to_be_bytes());

        let (_, data, cost) = self.handle_request(EvmApiMethod::AccountCode, &req);
        self.last_code = Some((address, data.clone()));
        (data, cost)
    }

    fn account_codehash(&mut self, address: Bytes20) -> (Bytes32, u64) {
        let (res, _, cost) = self.handle_request(EvmApiMethod::AccountCodeHash, address.as_slice());
        (res.try_into().unwrap(), cost)
    }

    fn add_pages(&mut self, pages: u16) -> u64 {
        self.handle_request(EvmApiMethod::AddPages, &pages.to_be_bytes())
            .2
    }

    fn capture_hostio(
        &mut self,
        name: &str,
        args: &[u8],
        outs: &[u8],
        start_ink: u64,
        end_ink: u64,
    ) {
        let mut request = Vec::with_capacity(2 * 8 + 3 * 2 + name.len() + args.len() + outs.len());
        request.extend(start_ink.to_be_bytes());
        request.extend(end_ink.to_be_bytes());
        request.extend((name.len() as u16).to_be_bytes());
        request.extend((args.len() as u16).to_be_bytes());
        request.extend((outs.len() as u16).to_be_bytes());
        request.extend(name.as_bytes());
        request.extend(args);
        request.extend(outs);
        self.handle_request(EvmApiMethod::CaptureHostIO, &request);
    }
}
