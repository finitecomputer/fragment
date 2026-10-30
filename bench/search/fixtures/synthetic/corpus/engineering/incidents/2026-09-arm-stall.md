# Incident: arm stall in the north block

## Summary

On 18 September 2026 thirty pickers in the north block stopped mid-reach for 47 minutes. The cause was a torque limit set in metric units read as imperial by the new gripper driver.

## Timeline

- 09:12 first stall alarm
- 09:20 Tomas Reyes declared an incident and became incident lead
- 09:59 firmware 4.2.1 reverted to 4.2.0 across the block

## Actions

- Add a unit test for torque limit parsing (owner: Imani Okafor)
- Canary new drivers on five pickers for a full day (owner: Wren Halvorsen)
