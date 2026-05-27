# callbook-rs Python Bindings

Python bindings for the `callbook-rs` native reader.

```python
from callbook_rs import CallBook

with CallBook.open("/path/to/hamcall-db") as db:
    with db.lookup("W1AW") as result:
        print(result.status)
        if result.current is not None:
            print(result.current.fields["grid"])

    with db.profile("W1AW") as profile:
        if profile.current is not None:
            print(profile.current.fields["city"])
        if profile.country is not None:
            print(profile.country.cleaned_name)
```

Wheels bundle the platform native library. You still need a local licensed
HamCall database installation; this package does not include database records.
