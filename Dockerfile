FROM rust AS builder
WORKDIR /app
COPY . .
ENV LISTEN_ON=0.0.0.0:80
RUN cargo build --release --locked


FROM ubuntu
WORKDIR /app
COPY --from=builder /app/target/release/songren /app/songren
ENTRYPOINT ["/app/songren"]
