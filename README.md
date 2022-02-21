Minimalistic PubSub WebSocket server written in Rust using [tokio.rs](https://tokio.rs/) and Redis.

### Run
```shell
./pubsub-server run --address 0.0.0.0 --port 3000 --jwt-secret secret --redis-address 127.0.0.1 --rest-api-key apikey --rest-api-port 3001 --rest-api-address 0.0.0.0
```

```shell
USAGE:                                                                                                                                                               
    pubsub-server.exe [OPTIONS] --port <PORT> --jwt-secret <JWT_SECRET> --redis-address <REDIS_ADDRESS> --rest-api-port <REST_API_PORT> --rest-api-key <REST_API_KEY>
                                                                                                                                                                     
OPTIONS:                                                                                                                                                             
        --address <ADDRESS>                                                                                                                                          
            WebSocket server listen address. 0.0.0.0 for all network interfaces                                                                                      

    -h, --help
            Print help information

        --jwt-secret <JWT_SECRET>
            JWT secret used to encode/decode client tokens

    -p, --port <PORT>
            WebSocket server listen port

        --redis-address <REDIS_ADDRESS>
            Redis address

        --rest-api-address <REST_API_ADDRESS>
            REST API listen address

        --rest-api-key <REST_API_KEY>


        --rest-api-port <REST_API_PORT>
            REST API listen port

    -V, --version
            Print version information
```

### Connect
When connecting to WebSocket server client need to provide a JWT token which contains list of topics client wants to subscribe:
```json
{
  "subs": ["topic1", "topic2"]
}
```

### Publish
Currently publishing can be done using REST API:
```shell
curl -X POST  -H 'Authorization: Bearer apikey' -H 'Content-Type: application/json' -d '{"channel": "test", "data": "message"}' http://127.0.0.1:3001/publish
```