[![Continuous Integration](https://github.com/helium/gwmp-mux/actions/workflows/rust.yml/badge.svg)](https://github.com/helium/gwmp-mux/actions/workflows/rust.yml)

# gwmp-mux

GWMP is a **G**ate**W**ay **M**essaging **P**rotocol used by LoRa packet
forwarders to typically talk to a LoRaWAN Network Server (LNS).

On the Helium's LoRaWAN Network, the GWMP is used to send packets to a [gateway
client](https://github.com/helium/gateway-rs).

This program, gwmp-mux, allows for a single packet forwarder connection to
be multiplexed out to one or more potential hosts. As such, a single gateway
can connect to multiple LNSs, for example.

