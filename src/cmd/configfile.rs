use handlebars::Handlebars;

use crate::config::Configuration;

pub fn run(config: &Configuration) {
    let template = r#"
# Logging settings.
[logging]

  # Log level.
  #
  # Valid options are:
  #   * TRACE
  #   * DEBUG
  #   * INFO
  #   * WARN
  #   * ERROR
  #   * OFF
  level="{{ logging.level }}"

  # Log to syslog.
  #
  # If set to true, log messages are being written to syslog instead of stdout.
  log_to_syslog={{ logging.log_to_syslog }}


# MQTT settings.
[mqtt]

  # Topic prefix.
  #
  # ChirpStack MQTT Forwarder publishes to the following topics:
  #
  #  * [Prefix/]gateway/[Gateway ID]/event/[Event]
  #  * [Prefix/]gateway/[Gateway ID]/state/[State]
  #
  # And subscribes to the following topic:
  #
  #  * [Prefix/]gateway/[Gateway ID]/command/[Command]
  #
  # The topic prefix can be used to define the region of the gateway.
  # Note, there is no need to add a trailing '/' to the prefix. The trailing
  # '/' is automatically added to the prefix if it is configured.
  topic_prefix="{{ mqtt.topic_prefix }}"

  # Use JSON encoding instead of Protobuf (binary).
  #
  # Note, only use this for debugging purposes.
  json={{ mqtt.json }}

  # MQTT server (e.g. scheme://host:port where scheme is tcp, ssl, ws or wss)
  server="{{ mqtt.server }}"

  # Connect with the given username (optional)
  username="{{ mqtt.username }}"

  # Connect with the given password (optional)
  password="{{ mqtt.password }}"

  # Quality of service level
  #
  # 0: at most once
  # 1: at least once
  # 2: exactly once
  #
  # Note: an increase of this value will decrease the performance.
  # For more information: https://www.hivemq.com/blog/mqtt-essentials-part-6-mqtt-quality-of-service-levels
  qos={{ mqtt.qos }}

  # Clean session
  #
  # Set the "clean session" flag in the connect message when this client
  # connects to an MQTT broker. By setting this flag you are indicating
  # that no messages saved by the broker for this client should be delivered.
  clean_session={{ mqtt.clean_session }}

  # Client ID
  #
  # Set the client id to be used by this client when connecting to the MQTT
  # broker. A client id must be no longer than 23 characters. If left blank,
  # a random id will be generated by ChirpStack.
  client_id="{{ mqtt.client_id }}"

  # Keep alive interval.
  #
  # This defines the maximum time that that should pass without communication
  # between the client and server.
  keep_alive_interval="{{ integration.mqtt.keep_alive_interval }}"

  # CA certificate file (optional)
  #
  # Use this when setting up a secure connection (when server uses ssl://...)
  # but the certificate used by the server is not trusted by any CA certificate
  # on the server (e.g. when self generated).
  ca_cert="{{ mqtt.ca_cert }}"

  # TLS certificate file (optional)
  tls_cert="{{ mqtt.tls_cert }}"

  # TLS key file (optional)
  tls_key="{{ mqtt.tls_key }}"

  # Reconnect interval.
  #
  # This defines the reconnection interval to the MQTT broker in case of
  # network issues.
  reconnect_interval="{{ integration.mqtt.reconnect_interval }}"


# Backend configuration.
[backend]

  # Enabled backend.
  #
  # Set this to the backend that must be used by the ChirpStack MQTT Forwarder.
  # Valid options are:
  #   * concentratord
  #   * semtech_udp
  enabled="{{ backend.enabled }}"

  # Filters.
  [backend.filters]

    # Forward CRC ok.
    forward_crc_ok={{ backend.filters.forward_crc_ok }}

    # Forward CRC invalid.
    forward_crc_invalid={{ backend.filters.forward_crc_invalid }}

    # Forward CRC missing.
    forward_crc_missing={{ backend.filters.forward_crc_missing }}

    # DevAddr prefix filters.
    #
    # Example configuration:
    # dev_addr_prefixes=["0000ff00/24"]
    #
    # The above filter means that the 24MSB of 0000ff00 will be used to
    # filter DevAddrs. Uplinks with DevAddrs that do not match any of the
    # configured filters will not be forwarded. Leaving this option empty
    # disables filtering on DevAddr.
    dev_addr_prefixes=[
      {{#each backend.filters.dev_addr_prefixes}}
      "{{this}}",
      {{/each}}
    ]

    # JoinEUI prefix filters.
    #
    # Example configuration:
    # join_eui_prefixes=["0000ff0000000000/24"]
    #
    # The above filter means that the 24MSB of 0000ff0000000000 will be used
    # to filter JoinEUIs. Uplinks with JoinEUIs that do not match any of the
    # configured filters will not be forwarded. Leaving this option empty
    # disables filtering on JoinEUI.
    join_eui_prefixes=[
      {{#each backend.filters.join_eui_prefixes}}
      "{{this}}",
      {{/each}}
    ]


  # ChirpStack Concentratord backend configuration.
  [backend.concentratord]

    # Event API URL.
    event_url="{{ backend.concentratord.event_url }}"

    # Command API URL.
    command_url="{{ backend.concentratord.command_url }}"


  # Semtech UDP backend configuration.
  [backend.semtech_udp]

    # ip:port to bind the UDP listener to.
    #
    # Example: 0.0.0.0:1700 to listen on port 1700 for all network interfaces.
    # This is the listener to which the packet-forwarder forwards its data
    # so make sure the 'serv_port_up' and 'serv_port_down' from your
    # packet-forwarder matches this port.
    bind="{{ backend.semtech_udp.bind }}"

    # Time fallback.
    #
    # In case the UDP packet-forwarder does not set the 'time' field, then the
    # server-time will be used as fallback if this option is enabled.
    time_fallback_enabled={{ backend.semtech_udp.time_fallback_enabled }}


# Gateway metadata configuration.
[metadata]

  # Static key / value metadata.
  [metadata.static]
      
    # Example:
    # serial_number="1234"
    {{#each metadata.static}}
    {{ @key }}="{{ this }}"
    {{/each}}


  # Commands returning metadata.
  [metadata.commands]

    # Example:
    # datetime=["date", "-R"]
    {{#each metadata.commands}}
    {{ @key }}=[
      {{#each this}}
      "{{ this }}",
      {{/each}}
    ]
    {{/each}}


# Executable commands.
[commands]

  # Example:
  # reboot=["/usr/bin/reboot"]
  {{#each commands}}
  {{ @key }}=[
    {{#each this}}
    "{{ this }}",
    {{/each}}
  ]
  {{/each}}


# Callback commands.
#
# These are commands that are triggered by certain events (e.g. MQTT connected
# or error). These commands are intended to e.g. trigger a LED of a gateway.
# Commands are configured as an array, where the first item is the path to the
# command, and the (optional) remaining elements are the arguments. An empty
# array disables the callback.
[callbacks]

  # On MQTT connected.
  on_mqtt_connected=[]

  # On MQTT connection error.
  on_mqtt_connection_error=[]
"#;

    let reg = Handlebars::new();
    println!(
        "{}",
        reg.render_template(template, config)
            .expect("Render configfile error")
    );
}
