#include "net/Protocol.hpp"
#include "sim/CommandQueue.hpp"

#include <cassert>
#include <cmath>
#include <cstdlib>
#include <vector>

int main() {
	using sim::AuthoritativeBody;

	{
		std::vector<std::uint8_t> buf;
		std::vector<AuthoritativeBody> bodiesIn;
		bodiesIn.push_back(AuthoritativeBody{
		    .id = 7,
		    .x = 1.0,
		    .y = 2.0,
		    .vx = 3.0,
		    .vy = 4.0,
		    .mass = 5.0,
		    .radius = 6.0,
		    .name = "alpha",
		});
		const std::uint64_t tick = 42;
		const std::uint64_t joinG = 9001;
		assert(net::writeJoinAccept(tick, joinG, bodiesIn, 7, 1.25, buf));

		std::uint64_t tickOut = 0;
		std::uint64_t joinGOut = 0;
		std::vector<AuthoritativeBody> bodiesOut;
		sim::BodyId own = 0;
		double ts = 0.0;
		assert(net::readJoinAccept(buf.data(), buf.size(), tickOut, joinGOut, bodiesOut, own, ts));
		assert(tickOut == tick);
		assert(joinGOut == joinG);
		assert(own == 7);
		assert(std::abs(ts - 1.25) < 1e-12);
		assert(bodiesOut.size() == 1);
		assert(bodiesOut[0].id == 7);
		assert(bodiesOut[0].name == "alpha");
	}

	{
		std::vector<std::uint8_t> buf;
		assert(net::writeShipState(3, 777, 9, 1.1, 2.2, 3.3, 4.4, 0.5f, 1, 88, buf));
		std::uint64_t tick = 0;
		std::uint64_t g = 0;
		sim::BodyId id = 0;
		double px = 0;
		double py = 0;
		double vx = 0;
		double vy = 0;
		float facing = 0.f;
		std::uint8_t tf = 0;
		std::uint8_t tp = 0;
		assert(net::readShipState(buf.data(), buf.size(), tick, g, id, px, py, vx, vy, facing, tf,
		                          tp));
		assert(tick == 3);
		assert(g == 777);
		assert(id == 9);
		assert(std::abs(px - 1.1) < 1e-12);
		assert(tf == 1);
		assert(tp == 88);
	}

	{
		std::vector<sim::AuthoritativeBody> bodiesIn;
		bodiesIn.push_back(sim::AuthoritativeBody{
		    .id = 1,
		    .x = 10.0,
		    .y = -1.0,
		    .vx = 0.1,
		    .vy = 0.3,
		    .mass = 11.0,
		    .radius = 3.0,
		    .name = "A",
		});
		bodiesIn.push_back(sim::AuthoritativeBody{
		    .id = 2,
		    .x = 20.0,
		    .y = -2.0,
		    .vx = 0.2,
		    .vy = 0.4,
		    .mass = 22.0,
		    .radius = 4.0,
		    .name = "B",
		});
		std::vector<std::uint8_t> buf;
		assert(net::writeWorldDynamicSnapshot(5, 12345, bodiesIn, buf));
		std::uint64_t tick = 0;
		std::uint64_t g = 0;
		std::vector<sim::AuthoritativeBody> bodiesOut;
		assert(net::readWorldDynamicSnapshot(buf.data(), buf.size(), tick, g, bodiesOut));
		assert(tick == 5);
		assert(g == 12345);
		assert(bodiesOut.size() == 2);
		assert(std::abs(bodiesOut[1].x - 20.0) < 1e-12);
		assert(std::abs(bodiesOut[0].vy - 0.3) < 1e-12);
		assert(std::abs(bodiesOut[0].mass - 11.0) < 1e-12);
		assert(std::abs(bodiesOut[1].radius - 4.0) < 1e-12);
		assert(bodiesOut[0].name == "A" && bodiesOut[1].name == "B");
	}

	{
		std::vector<AuthoritativeBody> bodiesIn;
		bodiesIn.push_back(AuthoritativeBody{
		    .id = 11,
		    .x = 1.0,
		    .y = 2.0,
		    .vx = 3.0,
		    .vy = 4.0,
		    .mass = 5.0,
		    .radius = 6.0,
		    .name = "late",
		});
		std::vector<std::uint8_t> buf;
		assert(net::writeAuthoritativeBodyUpsert(9, 8000, bodiesIn, buf));
		std::uint64_t tickOut = 0;
		std::uint64_t stepOut = 0;
		std::vector<AuthoritativeBody> bodiesOut;
		assert(
		    net::readAuthoritativeBodyUpsert(buf.data(), buf.size(), tickOut, stepOut, bodiesOut));
		assert(tickOut == 9);
		assert(stepOut == 8000);
		assert(bodiesOut.size() == 1);
		assert(bodiesOut[0].id == 11);
		assert(bodiesOut[0].name == "late");
	}

	return EXIT_SUCCESS;
}
