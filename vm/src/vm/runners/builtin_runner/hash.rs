use crate::air_private_input::{PrivateInput, PrivateInputPair};
use crate::stdlib::{cell::RefCell, prelude::*};
use crate::types::errors::math_errors::MathError;
use crate::types::instance_definitions::pedersen_instance_def::{
    CELLS_PER_HASH, INPUT_CELLS_PER_HASH,
};
use crate::types::relocatable::{MaybeRelocatable, Relocatable};
use crate::vm::errors::memory_errors::MemoryError;
use crate::vm::errors::runner_errors::RunnerError;
use crate::vm::runners::cairo_pie::BuiltinAdditionalData;
use crate::vm::vm_memory::memory::Memory;
use crate::vm::vm_memory::memory_segments::MemorySegmentManager;
use crate::Felt252;
use num_integer::{div_ceil, Integer};
use starknet_crypto::{pedersen_hash, FieldElement};

use super::HASH_BUILTIN_NAME;

#[derive(Debug, Clone)]
pub struct HashBuiltinRunner {
    pub base: usize,
    ratio: Option<u32>,
    pub(crate) cells_per_instance: u32,
    pub(crate) n_input_cells: u32,
    pub(crate) stop_ptr: Option<usize>,
    pub(crate) included: bool,
    pub(crate) instances_per_component: u32,
    // This act as a cache to optimize calls to deduce_memory_cell
    // Therefore need interior mutability
    // 1 at position 'n' means offset 'n' relative to base pointer
    // has been verified
    pub(self) verified_addresses: RefCell<Vec<bool>>,
}

impl HashBuiltinRunner {
    pub fn new(ratio: Option<u32>, included: bool) -> Self {
        HashBuiltinRunner {
            base: 0,
            ratio,
            cells_per_instance: CELLS_PER_HASH,
            n_input_cells: INPUT_CELLS_PER_HASH,
            stop_ptr: None,
            verified_addresses: RefCell::new(Vec::new()),
            included,
            instances_per_component: 1,
        }
    }

    pub fn initialize_segments(&mut self, segments: &mut MemorySegmentManager) {
        self.base = segments.add().segment_index as usize // segments.add() always returns a positive index
    }

    pub fn initial_stack(&self) -> Vec<MaybeRelocatable> {
        if self.included {
            vec![MaybeRelocatable::from((self.base as isize, 0))]
        } else {
            vec![]
        }
    }

    pub fn base(&self) -> usize {
        self.base
    }

    pub fn ratio(&self) -> Option<u32> {
        self.ratio
    }

    pub fn add_validation_rule(&self, _memory: &mut Memory) {}

    pub fn deduce_memory_cell(
        &self,
        address: Relocatable,
        memory: &Memory,
    ) -> Result<Option<MaybeRelocatable>, RunnerError> {
        if address
            .offset
            .mod_floor(&(self.cells_per_instance as usize))
            != 2
            || *self
                .verified_addresses
                .borrow()
                .get(address.offset)
                .unwrap_or(&false)
        {
            return Ok(None);
        };

        let num_a = memory.get(&MaybeRelocatable::RelocatableValue(Relocatable {
            segment_index: address.segment_index,
            offset: address.offset - 1,
        }));
        let num_b = memory.get(&MaybeRelocatable::RelocatableValue(Relocatable {
            segment_index: address.segment_index,
            offset: address.offset - 2,
        }));
        if let (Some(MaybeRelocatable::Int(num_a)), Some(MaybeRelocatable::Int(num_b))) = (
            num_a.as_ref().map(|x| x.as_ref()),
            num_b.as_ref().map(|x| x.as_ref()),
        ) {
            if self.verified_addresses.borrow().len() <= address.offset {
                self.verified_addresses
                    .borrow_mut()
                    .resize(address.offset + 1, false);
            }
            self.verified_addresses.borrow_mut()[address.offset] = true;

            //Convert MaybeRelocatable to FieldElement
            let a_be_bytes = num_a.to_bytes_be();
            let b_be_bytes = num_b.to_bytes_be();
            let (y, x) = match (
                FieldElement::from_bytes_be(&a_be_bytes),
                FieldElement::from_bytes_be(&b_be_bytes),
            ) {
                (Ok(field_element_a), Ok(field_element_b)) => (field_element_a, field_element_b),
                _ => return Err(MathError::ByteConversionError.into()),
            };
            //Compute pedersen Hash
            let fe_result = pedersen_hash(&x, &y);
            //Convert result from FieldElement to MaybeRelocatable
            let result = Felt252::from_bytes_be(&fe_result.to_bytes_be());
            return Ok(Some(MaybeRelocatable::from(result)));
        }
        Ok(None)
    }

    pub fn get_memory_segment_addresses(&self) -> (usize, Option<usize>) {
        (self.base, self.stop_ptr)
    }

    pub fn get_used_cells(&self, segments: &MemorySegmentManager) -> Result<usize, MemoryError> {
        segments
            .get_segment_used_size(self.base())
            .ok_or(MemoryError::MissingSegmentUsedSizes)
    }

    pub fn get_used_instances(
        &self,
        segments: &MemorySegmentManager,
    ) -> Result<usize, MemoryError> {
        let used_cells = self.get_used_cells(segments)?;
        Ok(div_ceil(used_cells, self.cells_per_instance as usize))
    }

    pub fn final_stack(
        &mut self,
        segments: &MemorySegmentManager,
        pointer: Relocatable,
    ) -> Result<Relocatable, RunnerError> {
        if self.included {
            let stop_pointer_addr = (pointer - 1)
                .map_err(|_| RunnerError::NoStopPointer(Box::new(HASH_BUILTIN_NAME)))?;
            let stop_pointer = segments
                .memory
                .get_relocatable(stop_pointer_addr)
                .map_err(|_| RunnerError::NoStopPointer(Box::new(HASH_BUILTIN_NAME)))?;
            if self.base as isize != stop_pointer.segment_index {
                return Err(RunnerError::InvalidStopPointerIndex(Box::new((
                    HASH_BUILTIN_NAME,
                    stop_pointer,
                    self.base,
                ))));
            }
            let stop_ptr = stop_pointer.offset;
            let num_instances = self.get_used_instances(segments)?;
            let used = num_instances * self.cells_per_instance as usize;
            if stop_ptr != used {
                return Err(RunnerError::InvalidStopPointer(Box::new((
                    HASH_BUILTIN_NAME,
                    Relocatable::from((self.base as isize, used)),
                    Relocatable::from((self.base as isize, stop_ptr)),
                ))));
            }
            self.stop_ptr = Some(stop_ptr);
            Ok(stop_pointer_addr)
        } else {
            self.stop_ptr = Some(0);
            Ok(pointer)
        }
    }

    pub fn get_additional_data(&self) -> BuiltinAdditionalData {
        let mut verified_addresses = Vec::new();
        for (offset, is_verified) in self.verified_addresses.borrow().iter().enumerate() {
            if *is_verified {
                verified_addresses.push(Relocatable::from((self.base as isize, offset)));
            }
        }
        BuiltinAdditionalData::Hash(verified_addresses)
    }

    pub fn air_private_input(&self, memory: &Memory) -> Vec<PrivateInput> {
        let mut private_inputs = vec![];
        if let Some(segment) = memory.data.get(self.base) {
            let segment_len = segment.len();
            for (index, off) in (0..segment_len)
                .step_by(CELLS_PER_HASH as usize)
                .enumerate()
            {
                // Add the input cells of each hash instance to the private inputs
                if let (Ok(x), Ok(y)) = (
                    memory.get_integer((self.base as isize, off).into()),
                    memory.get_integer((self.base as isize, off + 1).into()),
                ) {
                    private_inputs.push(PrivateInput::Pair(PrivateInputPair {
                        index,
                        x: *x,
                        y: *y,
                    }))
                }
            }
        }
        private_inputs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hint_processor::builtin_hint_processor::builtin_hint_processor_definition::BuiltinHintProcessor;
    use crate::serde::deserialize_program::BuiltinName;
    use crate::stdlib::collections::HashMap;
    use crate::types::program::Program;
    use crate::utils::test_utils::*;
    use crate::vm::runners::cairo_runner::CairoRunner;
    use crate::{felt_hex, relocatable};

    use crate::vm::{
        errors::memory_errors::MemoryError, runners::builtin_runner::BuiltinRunner,
        vm_core::VirtualMachine,
    };

    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::*;

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn get_used_instances() {
        let builtin = HashBuiltinRunner::new(Some(10), true);

        let mut vm = vm!();
        vm.segments.segment_used_sizes = Some(vec![1]);

        assert_eq!(builtin.get_used_instances(&vm.segments), Ok(1));
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn final_stack() {
        let mut builtin = HashBuiltinRunner::new(Some(10), true);

        let mut vm = vm!();

        vm.segments = segments![
            ((0, 0), (0, 0)),
            ((0, 1), (0, 1)),
            ((2, 0), (0, 0)),
            ((2, 1), (0, 0))
        ];

        vm.segments.segment_used_sizes = Some(vec![0]);

        let pointer = Relocatable::from((2, 2));

        assert_eq!(
            builtin.final_stack(&vm.segments, pointer).unwrap(),
            Relocatable::from((2, 1))
        );
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn final_stack_error_stop_pointer() {
        let mut builtin = HashBuiltinRunner::new(Some(10), true);

        let mut vm = vm!();

        vm.segments = segments![
            ((0, 0), (0, 0)),
            ((0, 1), (0, 1)),
            ((2, 0), (0, 0)),
            ((2, 1), (0, 0))
        ];

        vm.segments.segment_used_sizes = Some(vec![999]);

        let pointer = Relocatable::from((2, 2));

        assert_eq!(
            builtin.final_stack(&vm.segments, pointer),
            Err(RunnerError::InvalidStopPointer(Box::new((
                HASH_BUILTIN_NAME,
                relocatable!(0, 999),
                relocatable!(0, 0)
            ))))
        );
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn final_stack_error_when_not_included() {
        let mut builtin = HashBuiltinRunner::new(Some(10), false);

        let mut vm = vm!();

        vm.segments = segments![
            ((0, 0), (0, 0)),
            ((0, 1), (0, 1)),
            ((2, 0), (0, 0)),
            ((2, 1), (0, 0))
        ];

        vm.segments.segment_used_sizes = Some(vec![0]);

        let pointer = Relocatable::from((2, 2));

        assert_eq!(
            builtin.final_stack(&vm.segments, pointer).unwrap(),
            Relocatable::from((2, 2))
        );
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn final_stack_error_non_relocatable() {
        let mut builtin = HashBuiltinRunner::new(Some(10), true);

        let mut vm = vm!();

        vm.segments = segments![
            ((0, 0), (0, 0)),
            ((0, 1), (0, 1)),
            ((2, 0), (0, 0)),
            ((2, 1), 2)
        ];

        vm.segments.segment_used_sizes = Some(vec![0]);

        let pointer = Relocatable::from((2, 2));

        assert_eq!(
            builtin.final_stack(&vm.segments, pointer),
            Err(RunnerError::NoStopPointer(Box::new(HASH_BUILTIN_NAME)))
        );
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn get_used_cells_and_allocated_size_test() {
        let builtin: BuiltinRunner = HashBuiltinRunner::new(Some(10), true).into();

        let mut vm = vm!();

        vm.segments.segment_used_sizes = Some(vec![0]);

        let program = program!(
            builtins = vec![BuiltinName::ec_op],
            data = vec_data!(
                (4612671182993129469_i64),
                (5189976364521848832_i64),
                (18446744073709551615_i128),
                (5199546496550207487_i64),
                (4612389712311386111_i64),
                (5198983563776393216_i64),
                (2),
                (2345108766317314046_i64),
                (5191102247248822272_i64),
                (5189976364521848832_i64),
                (7),
                (1226245742482522112_i64),
                ((
                    "3618502788666131213697322783095070105623107215331596699973092056135872020470",
                    10
                )),
                (2345108766317314046_i64)
            ),
            main = Some(8),
        );

        let mut cairo_runner = cairo_runner!(program);

        let mut hint_processor = BuiltinHintProcessor::new_empty();

        let address = cairo_runner.initialize(&mut vm).unwrap();

        cairo_runner
            .run_until_pc(address, &mut vm, &mut hint_processor)
            .unwrap();

        assert_eq!(builtin.get_used_cells_and_allocated_size(&vm), Ok((0, 3)));
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn get_allocated_memory_units() {
        let builtin: BuiltinRunner = HashBuiltinRunner::new(Some(10), true).into();

        let mut vm = vm!();

        let program = program!(
            builtins = vec![BuiltinName::ec_op],
            data = vec_data!(
                (4612671182993129469_i64),
                (5189976364521848832_i64),
                (18446744073709551615_i128),
                (5199546496550207487_i64),
                (4612389712311386111_i64),
                (5198983563776393216_i64),
                (2),
                (2345108766317314046_i64),
                (5191102247248822272_i64),
                (5189976364521848832_i64),
                (7),
                (1226245742482522112_i64),
                ((
                    "3618502788666131213697322783095070105623107215331596699973092056135872020470",
                    10
                )),
                (2345108766317314046_i64)
            ),
            main = Some(8),
        );

        let mut cairo_runner = cairo_runner!(program);

        let mut hint_processor = BuiltinHintProcessor::new_empty();

        let address = cairo_runner.initialize(&mut vm).unwrap();

        cairo_runner
            .run_until_pc(address, &mut vm, &mut hint_processor)
            .unwrap();

        assert_eq!(builtin.get_allocated_memory_units(&vm), Ok(3));
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn deduce_memory_cell_pedersen_for_preset_memory_valid() {
        let memory = memory![((0, 3), 32), ((0, 4), 72), ((0, 5), 0)];
        let builtin = HashBuiltinRunner::new(Some(8), true);

        let result = builtin.deduce_memory_cell(Relocatable::from((0, 5)), &memory);
        assert_eq!(
            result,
            Ok(Some(MaybeRelocatable::from(felt_hex!(
                "0x73b3ec210cccbb970f80c6826fb1c40ae9f487617696234ff147451405c339f"
            ))))
        );
        assert_eq!(
            builtin.verified_addresses.into_inner(),
            vec![false, false, false, false, false, true],
        );
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn deduce_memory_cell_pedersen_for_preset_memory_incorrect_offset() {
        let memory = memory![((0, 4), 32), ((0, 5), 72), ((0, 6), 0)];
        let builtin = HashBuiltinRunner::new(Some(8), true);
        let result = builtin.deduce_memory_cell(Relocatable::from((0, 6)), &memory);
        assert_eq!(result, Ok(None));
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn deduce_memory_cell_pedersen_for_preset_memory_no_values_to_hash() {
        let memory = memory![((0, 4), 72), ((0, 5), 0)];
        let builtin = HashBuiltinRunner::new(Some(8), true);
        let result = builtin.deduce_memory_cell(Relocatable::from((0, 5)), &memory);
        assert_eq!(result, Ok(None));
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn deduce_memory_cell_pedersen_for_preset_memory_already_computed() {
        let memory = memory![((0, 3), 32), ((0, 4), 72), ((0, 5), 0)];
        let mut builtin = HashBuiltinRunner::new(Some(8), true);
        builtin.verified_addresses = RefCell::new(vec![false, false, false, false, false, true]);
        let result = builtin.deduce_memory_cell(Relocatable::from((0, 5)), &memory);
        assert_eq!(result, Ok(None));
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn get_memory_segment_addresses() {
        let builtin = HashBuiltinRunner::new(Some(256), true);

        assert_eq!(builtin.get_memory_segment_addresses(), (0, None),);
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn get_memory_accesses_missing_segment_used_sizes() {
        let builtin = BuiltinRunner::Hash(HashBuiltinRunner::new(Some(256), true));
        let vm = vm!();

        assert_eq!(
            builtin.get_memory_accesses(&vm),
            Err(MemoryError::MissingSegmentUsedSizes),
        );
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn get_memory_accesses_empty() {
        let builtin = BuiltinRunner::Hash(HashBuiltinRunner::new(Some(256), true));
        let mut vm = vm!();

        vm.segments.segment_used_sizes = Some(vec![0]);
        assert_eq!(builtin.get_memory_accesses(&vm), Ok(vec![]));
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn get_memory_accesses() {
        let builtin = BuiltinRunner::Hash(HashBuiltinRunner::new(Some(256), true));
        let mut vm = vm!();

        vm.segments.segment_used_sizes = Some(vec![4]);
        assert_eq!(
            builtin.get_memory_accesses(&vm),
            Ok(vec![
                (builtin.base() as isize, 0).into(),
                (builtin.base() as isize, 1).into(),
                (builtin.base() as isize, 2).into(),
                (builtin.base() as isize, 3).into(),
            ]),
        );
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn get_used_cells_missing_segment_used_sizes() {
        let builtin = BuiltinRunner::Hash(HashBuiltinRunner::new(Some(256), true));
        let vm = vm!();

        assert_eq!(
            builtin.get_used_cells(&vm.segments),
            Err(MemoryError::MissingSegmentUsedSizes)
        );
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn get_used_cells_empty() {
        let builtin = BuiltinRunner::Hash(HashBuiltinRunner::new(Some(256), true));
        let mut vm = vm!();

        vm.segments.segment_used_sizes = Some(vec![0]);
        assert_eq!(builtin.get_used_cells(&vm.segments), Ok(0));
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn get_used_cells() {
        let builtin = BuiltinRunner::Hash(HashBuiltinRunner::new(Some(256), true));
        let mut vm = vm!();

        vm.segments.segment_used_sizes = Some(vec![4]);
        assert_eq!(builtin.get_used_cells(&vm.segments), Ok(4));
    }

    #[test]
    fn get_additional_info() {
        let mut builtin = HashBuiltinRunner::new(Some(1), true);
        let verified_addresses = vec![Relocatable::from((0, 3)), Relocatable::from((0, 6))];
        builtin.verified_addresses =
            RefCell::new(vec![false, false, false, true, false, false, true]);
        assert_eq!(
            builtin.get_additional_data(),
            BuiltinAdditionalData::Hash(verified_addresses)
        )
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
    fn get_air_private_input() {
        let builtin: BuiltinRunner = HashBuiltinRunner::new(None, true).into();

        let memory = memory![
            ((0, 0), 0),
            ((0, 1), 1),
            ((0, 2), 2),
            ((0, 3), 3),
            ((0, 4), 4),
            ((0, 5), 5),
            ((0, 6), 6),
            ((0, 7), 7),
            ((0, 8), 8),
            ((0, 9), 9)
        ];
        assert_eq!(
            builtin.air_private_input(&memory),
            (vec![
                PrivateInput::Pair(PrivateInputPair {
                    index: 0,
                    x: 0.into(),
                    y: 1.into()
                }),
                PrivateInput::Pair(PrivateInputPair {
                    index: 1,
                    x: 3.into(),
                    y: 4.into()
                }),
                PrivateInput::Pair(PrivateInputPair {
                    index: 2,
                    x: 6.into(),
                    y: 7.into()
                }),
            ]),
        );
    }
}
