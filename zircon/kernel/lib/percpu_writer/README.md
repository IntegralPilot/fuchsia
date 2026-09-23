# percpu_writer

A library wrapping //zircon/kernel/lib/spsc_buffer with:

- Managing one buffer per cpu
- FXT serialization directly into the buffer
- Dropped record tracking (Emitting an FXT duration event detailing when records were dropped and
  how much data was lost).
