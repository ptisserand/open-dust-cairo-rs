use crate::bigint;
use crate::relocatable;
use crate::serde::deserialize_program::ApTracking;
use crate::types::exec_scope::ExecutionScopesProxy;
use crate::types::relocatable::Relocatable;
use crate::types::{instruction::Register, relocatable::MaybeRelocatable};
use crate::vm::runners::builtin_runner::BuiltinRunner;
use crate::vm::vm_core::VMProxy;
use crate::vm::vm_memory::memory::MemoryProxy;
use crate::vm::{
    context::run_context::RunContext, errors::vm_errors::VirtualMachineError,
    hints::execute_hint::HintReference, runners::builtin_runner::RangeCheckBuiltinRunner,
};
use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive};
use std::any::Any;
use std::collections::HashMap;

//Tries to convert a BigInt value to usize
pub fn bigint_to_usize(bigint: &BigInt) -> Result<usize, VirtualMachineError> {
    bigint
        .to_usize()
        .ok_or(VirtualMachineError::BigintToUsizeFail)
}

//Tries to convert a BigInt value to U32
pub fn bigint_to_u32(bigint: &BigInt) -> Result<u32, VirtualMachineError> {
    bigint.to_u32().ok_or(VirtualMachineError::BigintToU32Fail)
}

//Returns a reference to the  RangeCheckBuiltinRunner struct if range_check builtin is present
pub fn get_range_check_builtin(
    builtin_runners: &Vec<(String, Box<dyn BuiltinRunner>)>,
) -> Result<&RangeCheckBuiltinRunner, VirtualMachineError> {
    for (name, builtin) in builtin_runners {
        if name == &String::from("range_check") {
            if let Some(range_check_builtin) =
                builtin.as_any().downcast_ref::<RangeCheckBuiltinRunner>()
            {
                return Ok(range_check_builtin);
            };
        }
    }
    Err(VirtualMachineError::NoRangeCheckBuiltin)
}

pub fn get_ptr_from_var_name(
    var_name: &str,
    vm_proxy: &VMProxy,
    ids_data: &HashMap<String, HintReference>,
    ap_tracking: &ApTracking,
) -> Result<Relocatable, VirtualMachineError> {
    let var_addr = get_relocatable_from_var_name(var_name, vm_proxy, ids_data, ap_tracking)?;
    //Add immediate if present in reference
    let hint_reference = ids_data
        .get(&String::from(var_name))
        .ok_or(VirtualMachineError::FailedToGetIds)?;
    if hint_reference.dereference {
        let value = vm_proxy.memory.get_relocatable(&var_addr)?;
        if let Some(immediate) = &hint_reference.immediate {
            let modified_value = relocatable!(
                value.segment_index,
                value.offset + bigint_to_usize(immediate)?
            );
            Ok(modified_value)
        } else {
            Ok(value.clone())
        }
    } else {
        Ok(var_addr)
    }
}

fn apply_ap_tracking_correction(
    ap: &Relocatable,
    ref_ap_tracking: &ApTracking,
    hint_ap_tracking: &ApTracking,
) -> Result<MaybeRelocatable, VirtualMachineError> {
    // check that both groups are the same
    if ref_ap_tracking.group != hint_ap_tracking.group {
        return Err(VirtualMachineError::InvalidTrackingGroup(
            ref_ap_tracking.group,
            hint_ap_tracking.group,
        ));
    }
    let ap_diff = hint_ap_tracking.offset - ref_ap_tracking.offset;

    Ok(MaybeRelocatable::from((
        ap.segment_index,
        ap.offset - ap_diff,
    )))
}

///Computes the memory address indicated by the HintReference
pub fn compute_addr_from_reference(
    hint_reference: &HintReference,
    run_context: &RunContext,
    memory: &MemoryProxy,
    //TODO: Check if this option is necessary
    hint_ap_tracking: Option<&ApTracking>,
    //TODO: Change this to Result
) -> Result<Option<MaybeRelocatable>, VirtualMachineError> {
    let base_addr = match hint_reference.register {
        Register::FP => run_context.fp.clone(),
        Register::AP => {
            if hint_ap_tracking.is_none() || hint_reference.ap_tracking_data.is_none() {
                return Err(VirtualMachineError::NoneApTrackingData);
            }

            if let MaybeRelocatable::RelocatableValue(ref relocatable) = run_context.ap {
                apply_ap_tracking_correction(
                    relocatable,
                    // it is safe to call these unrwaps here, since it has been checked
                    // they are not None's
                    // this could be refactored to use pattern match but it will be
                    // unnecesarily verbose
                    hint_reference.ap_tracking_data.as_ref().unwrap(),
                    hint_ap_tracking.unwrap(),
                )?
            } else {
                return Err(VirtualMachineError::InvalidApValue(run_context.ap.clone()));
            }
        }
    };

    if let MaybeRelocatable::RelocatableValue(relocatable) = base_addr {
        if hint_reference.offset1.is_negative()
            && relocatable.offset < hint_reference.offset1.abs() as usize
        {
            return Ok(None);
        }
        if !hint_reference.inner_dereference {
            return Ok(Some(MaybeRelocatable::from((
                relocatable.segment_index,
                (relocatable.offset as i32 + hint_reference.offset1 + hint_reference.offset2)
                    as usize,
            ))));
        } else {
            let addr = MaybeRelocatable::from((
                relocatable.segment_index,
                (relocatable.offset as i32 + hint_reference.offset1) as usize,
            ));

            match memory.get(&addr) {
                Ok(Some(&MaybeRelocatable::RelocatableValue(ref dereferenced_addr))) => {
                    if let Some(imm) = &hint_reference.immediate {
                        return Ok(Some(MaybeRelocatable::from((
                            dereferenced_addr.segment_index,
                            dereferenced_addr.offset
                                + imm
                                    .to_usize()
                                    .ok_or(VirtualMachineError::BigintToUsizeFail)?,
                        ))));
                    } else {
                        return Ok(Some(MaybeRelocatable::from((
                            dereferenced_addr.segment_index,
                            (dereferenced_addr.offset as i32 + hint_reference.offset2) as usize,
                        ))));
                    }
                }

                _none_or_error => return Ok(None),
            }
        }
    }

    Ok(None)
}

pub fn get_address_from_var_name(
    var_name: &str,
    vm_proxy: &VMProxy,
    ids_data: &HashMap<String, HintReference>,
    ap_tracking: &ApTracking,
) -> Result<MaybeRelocatable, VirtualMachineError> {
    compute_addr_from_reference(
        ids_data
            .get(var_name)
            .ok_or(VirtualMachineError::FailedToGetIds)?,
        vm_proxy.run_context,
        &vm_proxy.memory,
        Some(ap_tracking),
    )?
    .ok_or(VirtualMachineError::FailedToGetIds)
}

pub fn insert_value_from_var_name(
    var_name: &str,
    value: impl Into<MaybeRelocatable>,
    vm_proxy: &mut VMProxy,
    ids_data: &HashMap<String, HintReference>,
    ap_tracking: &ApTracking,
) -> Result<(), VirtualMachineError> {
    let var_address = get_relocatable_from_var_name(var_name, vm_proxy, ids_data, ap_tracking)?;
    vm_proxy.memory.insert_value(&var_address, value)
}

//Inserts value into ap
pub fn insert_value_into_ap(
    memory: &mut MemoryProxy,
    run_context: &RunContext,
    value: impl Into<MaybeRelocatable>,
) -> Result<(), VirtualMachineError> {
    memory.insert_value(
        &(run_context
            .ap
            .clone()
            .try_into()
            .map_err(VirtualMachineError::MemoryError)?),
        value,
    )
}

//Gets the address of a variable name.
//If the address is an MaybeRelocatable::Relocatable(Relocatable) return Relocatable
//else raises Err
pub fn get_relocatable_from_var_name(
    var_name: &str,
    vm_proxy: &VMProxy,
    ids_data: &HashMap<String, HintReference>,
    ap_tracking: &ApTracking,
) -> Result<Relocatable, VirtualMachineError> {
    match get_address_from_var_name(var_name, vm_proxy, ids_data, ap_tracking)? {
        MaybeRelocatable::RelocatableValue(relocatable) => Ok(relocatable),
        address => Err(VirtualMachineError::ExpectedRelocatable(address)),
    }
}

//Gets the value of a variable name.
//If the value is an MaybeRelocatable::Int(Bigint) return &Bigint
//else raises Err
pub fn get_integer_from_var_name<'a>(
    var_name: &str,
    vm_proxy: &'a VMProxy,
    ids_data: &HashMap<String, HintReference>,
    ap_tracking: &ApTracking,
) -> Result<&'a BigInt, VirtualMachineError> {
    let relocatable = get_relocatable_from_var_name(var_name, vm_proxy, ids_data, ap_tracking)?;
    vm_proxy.memory.get_integer(&relocatable)
}

///Implements hint: memory[ap] = segments.add()
pub fn add_segment(vm_proxy: &mut VMProxy) -> Result<(), VirtualMachineError> {
    let new_segment_base = vm_proxy.memory.add_segment(vm_proxy.segments);
    insert_value_into_ap(&mut vm_proxy.memory, vm_proxy.run_context, new_segment_base)
}

//Implements hint: vm_enter_scope()
pub fn enter_scope(
    exec_scopes_proxy: &mut ExecutionScopesProxy,
) -> Result<(), VirtualMachineError> {
    exec_scopes_proxy.enter_scope(HashMap::new());
    Ok(())
}

//  Implements hint:
//  %{ vm_exit_scope() %}
pub fn exit_scope(exec_scopes_proxy: &mut ExecutionScopesProxy) -> Result<(), VirtualMachineError> {
    exec_scopes_proxy
        .exit_scope()
        .map_err(VirtualMachineError::MainScopeError)
}

//  Implements hint:
//  %{ vm_enter_scope({'n': ids.len}) %}
pub fn memcpy_enter_scope(
    vm_proxy: &mut VMProxy,
    exec_scopes_proxy: &mut ExecutionScopesProxy,
    ids_data: &HashMap<String, HintReference>,
    ap_tracking: &ApTracking,
) -> Result<(), VirtualMachineError> {
    let len: Box<dyn Any> =
        Box::new(get_integer_from_var_name("len", &vm_proxy, ids_data, ap_tracking)?.clone());
    exec_scopes_proxy.enter_scope(HashMap::from([(String::from("n"), len)]));
    Ok(())
}

// Implements hint:
// %{
//     n -= 1
//     ids.continue_copying = 1 if n > 0 else 0
// %}
pub fn memcpy_continue_copying(
    vm_proxy: &mut VMProxy,
    exec_scopes_proxy: &mut ExecutionScopesProxy,
    ids_data: &HashMap<String, HintReference>,
    ap_tracking: &ApTracking,
) -> Result<(), VirtualMachineError> {
    // get `n` variable from vm scope
    let n = exec_scopes_proxy.get_int_ref("n")?;
    // this variable will hold the value of `n - 1`
    let new_n = n - 1_i32;
    // if it is positive, insert 1 in the address of `continue_copying`
    // else, insert 0
    if new_n.is_positive() {
        insert_value_from_var_name(
            "continue_copying",
            bigint!(1),
            vm_proxy,
            ids_data,
            ap_tracking,
        )?;
    } else {
        insert_value_from_var_name(
            "continue_copying",
            bigint!(0),
            vm_proxy,
            ids_data,
            ap_tracking,
        )?;
    }
    exec_scopes_proxy.insert_value("n", new_n);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_utils::*;
    use crate::vm::hints::execute_hint::get_vm_proxy;
    use crate::vm::vm_core::VirtualMachine;
    use num_bigint::Sign;

    #[test]
    fn get_integer_from_var_name_valid() {
        let mut vm = vm!();
        // initialize memory segments
        vm.segments.add(&mut vm.memory, None);

        // initialize fp
        vm.run_context.fp = MaybeRelocatable::from((0, 1));

        let var_name: &str = "variable";

        //Create ids_data
        let mut ids_data = ids_data![var_name];

        //Insert ids.prev_locs.exp into memory
        vm.memory
            .insert(
                &MaybeRelocatable::from((0, 0)),
                &MaybeRelocatable::from(bigint!(10)),
            )
            .unwrap();
        let vm_proxy = get_vm_proxy(&mut vm);
        assert_eq!(
            get_integer_from_var_name(var_name, &vm_proxy, &ids_data, &ApTracking::default()),
            Ok(&bigint!(10))
        );
    }

    #[test]
    fn get_integer_from_var_name_invalid_expected_integer() {
        let mut vm = vm!();
        // initialize memory segments
        vm.segments.add(&mut vm.memory, None);

        // initialize fp
        vm.run_context.fp = MaybeRelocatable::from((0, 1));

        let var_name: &str = "variable";

        //Create ids_data
        let mut ids_data = ids_data![var_name];

        //Insert ids.variable into memory as a RelocatableValue
        vm.memory
            .insert(
                &MaybeRelocatable::from((0, 0)),
                &MaybeRelocatable::from((0, 1)),
            )
            .unwrap();
        let vm_proxy = &mut get_vm_proxy(&mut vm);
        assert_eq!(
            get_integer_from_var_name(var_name, &vm_proxy, &ids_data, &ApTracking::default()),
            Err(VirtualMachineError::ExpectedInteger(
                MaybeRelocatable::from((0, 0))
            ))
        );
    }
}
