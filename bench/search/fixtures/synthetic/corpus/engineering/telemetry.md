# Telemetry pipeline

Pickers stream joint torques, battery state and camera health every five seconds to the shed gateway, which batches them to the warehouse every minute. Retention is ninety days for raw samples and two years for hourly rollups.

## Gaps

When the shed gateway loses its uplink, samples queue on the gateway for up to six hours.
