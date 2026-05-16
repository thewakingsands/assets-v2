FROM golang:1.22-alpine AS build

WORKDIR /src

COPY go.mod ./
COPY main.go ./

RUN CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build -trimpath -ldflags="-s -w" -o /out/assets-server .

FROM gcr.io/distroless/static-debian12:nonroot

WORKDIR /app

COPY --from=build /out/assets-server /app/assets-server

ENV ASSETS_DIR=/data
ENV ASSETS_SERVER_ADDR=:8080

EXPOSE 8080

ENTRYPOINT ["/app/assets-server"]
