from starkware.cairo.common.dict import dict_read, dict_write
from starkware.cairo.common.default_dict import default_dict_new
from starkware.cairo.common.dict_access import DictAccess


func main():
    alloc_locals
    let (local my_dict : DictAccess*) = default_dict_new(17)
    dict_write{dict_ptr=my_dict}(key=12, new_value=34)
    let (local val1 : felt) = dict_read{dict_ptr=my_dict}(key=12)
    assert val1 = 34
    let (local val2 : felt) = dict_read{dict_ptr=my_dict}(key=11)
    assert val2 = 17
    let (local val3 : felt) = dict_read{dict_ptr=my_dict}(key=12)
    assert val3 = 34
    let (local val4 : felt) = dict_read{dict_ptr=my_dict}(key=11)
    assert val4 = 17
    return()
end
