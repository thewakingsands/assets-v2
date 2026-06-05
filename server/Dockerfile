FROM golang:1.24-alpine AS build

WORKDIR /src

COPY go.mod go.sum ./
COPY main.go ./

RUN CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build -trimpath -ldflags="-s -w" -o /out/assets-server .

FROM gcr.io/distroless/static-debian12:nonroot

WORKDIR /app

COPY --from=build /out/assets-server /app/assets-server

ENV ASSETS_CACHE_DIR=/data/cache
ENV ASSETS_SERVER=sdo
ENV ASSETS_SERVER_ADDR=:8080
ENV ASSETS_VERSION=
ENV MINIO_ENDPOINT=
ENV MINIO_ACCESS_KEY=
ENV MINIO_SECRET_KEY=
ENV MINIO_BUCKET_NAME=
ENV MINIO_PREFIX=
ENV MINIO_USE_SSL=true

VOLUME ["/data/cache"]

EXPOSE 8080

ENTRYPOINT ["/app/assets-server"]
