#include "compute.hpp"
#include <openssl/sha.h>
#include <algorithm>
#include <cctype>
#include <iomanip>
#include <sstream>
Computation compute(const std::string& operation, const std::string& payload) {
    if(payload.size() > 65536) return {false, "", "payload exceeds 64 KiB"};
    if(operation == "echo") return {true, payload, ""};
    if(operation == "uppercase") {
        auto output = payload;
        std::transform(output.begin(), output.end(), output.begin(),
            [](unsigned char c) {return static_cast<char>(std::toupper(c));});
        return {true, output, ""};
    }
    if(operation == "sha256") {
        unsigned char digest[SHA256_DIGEST_LENGTH];
        SHA256(reinterpret_cast<const unsigned char*>(payload.data()), payload.size(), digest);
        std::ostringstream out;
        for(auto byte: digest) out << std::hex << std::setw(2) << std::setfill('0') << static_cast<unsigned int>(byte);
        return {true, out.str(), ""};
    }
    return {false, "", "unsupported operation"};
}
