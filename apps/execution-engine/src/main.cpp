#include "compute.hpp"
#include "execution.grpc.pb.h"
#include <grpcpp/grpcpp.h>
#include <iostream>
class ExecutionService final : public aether::v1::ExecutionService::Service {
 grpc::Status Execute(grpc::ServerContext*, const aether::v1::ExecuteRequest* req, aether::v1::ExecuteResponse* res) override {
   auto result = compute(req->operation(), req->payload());
   res->set_task_id(req->task_id()); res->set_output(result.output);
   res->set_success(result.success); res->set_error(result.error);
   return grpc::Status::OK;
 }
 grpc::Status Health(grpc::ServerContext*, const aether::v1::HealthRequest*, aether::v1::HealthResponse* res) override {
   res->set_ready(true); return grpc::Status::OK;
 }
};
int main() {
 ExecutionService service;
 grpc::ServerBuilder builder;
 builder.AddListeningPort("0.0.0.0:50051", grpc::InsecureServerCredentials());
 builder.RegisterService(&service);
 auto server = builder.BuildAndStart();
 if(!server) {std::cerr << "Failed to start gRPC service\n";return 1;}
 std::cout << "aether execution engine listening on 50051\n";
 server->Wait();
}
